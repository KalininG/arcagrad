//! Debounced filesystem watcher for incremental scans.

use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::event::{AccessKind, AccessMode, EventKind, ModifyKind, RemoveKind};
use notify::RecursiveMode;
use notify_debouncer_full::{new_debouncer, DebounceEventResult};
use tokio_util::sync::CancellationToken;

use crate::server::jobs;
use crate::{scanner, AppState};

const DEBOUNCE: Duration = Duration::from_secs(2);

#[derive(Default)]
struct Signal {
    paths: Vec<PathBuf>,
    full: bool,
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with('.'))
}

/// Detect folder changes that cannot be reconciled by archive path.
fn is_directory_structural_change(kind: &EventKind, path: &Path) -> bool {
    if is_hidden(path) || scanner::is_archive(path) {
        return false;
    }
    matches!(
        kind,
        EventKind::Remove(RemoveKind::Folder)
            | EventKind::Remove(RemoveKind::Any)
            | EventKind::Modify(ModifyKind::Name(_))
    )
}

/// Ignore read/metadata echoes but accept a writer's close signal.
fn is_content_change(kind: &EventKind) -> bool {
    match kind {
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Access(_) | EventKind::Modify(ModifyKind::Metadata(_)) => false,
        _ => true,
    }
}

/// Start the best-effort content watcher.
pub fn spawn(state: AppState, cancel: CancellationToken) {
    if !state.config.watch {
        tracing::info!("file watching disabled (ARCA_WATCH)");
        return;
    }
    let content = state.config.content_dir.clone();

    // The notify callback runs outside Tokio.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Signal>();

    let mut debouncer = match new_debouncer(DEBOUNCE, None, move |res: DebounceEventResult| {
        if let Ok(events) = res {
            let mut sig = Signal::default();
            for e in events.iter().filter(|e| is_content_change(&e.kind)) {
                for p in &e.paths {
                    if scanner::is_archive(p) {
                        sig.paths.push(p.clone());
                    }
                    if is_directory_structural_change(&e.kind, p) {
                        sig.full = true;
                    }
                }
            }
            if sig.full || !sig.paths.is_empty() {
                let _ = tx.send(sig);
            }
        }
    }) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("file watcher disabled (init failed): {e}");
            return;
        }
    };

    if let Err(e) = debouncer.watch(&content, RecursiveMode::Recursive) {
        tracing::warn!(
            "file watcher disabled (cannot watch {}): {e}",
            content.display()
        );
        return;
    }

    // Dropping the debouncer stops the watch.
    std::mem::forget(debouncer);

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                msg = rx.recv() => {
                    let Some(mut sig) = msg else { break };
                    // Merge batches received while another job was queued.
                    while let Ok(more) = rx.try_recv() {
                        sig.full |= more.full;
                        sig.paths.extend(more.paths);
                    }

                    // Directory changes require the authoritative full scan.
                    if sig.full {
                        match jobs::enqueue_full_scan(&state.write).await {
                            Ok(Some(id)) => tracing::info!("directory change; queued full scan job {id}"),
                            Ok(None) => tracing::debug!("directory change; upgraded/merged into the pending scan"),
                            Err(e) => tracing::error!("watcher: enqueue full scan failed: {e:#}"),
                        }
                        continue;
                    }

                    let mut paths = sig.paths;
                    paths.sort();
                    paths.dedup();
                    // On lookup failure, scan every path rather than lose an event.
                    let to_scan = match scanner::changed_paths(
                        &state.read,
                        &state.config.content_dir,
                        &paths,
                    )
                    .await
                    {
                        Ok(changed) => changed,
                        Err(e) => {
                            tracing::warn!("watcher: change-check failed ({e:#}); scanning all flagged paths");
                            paths.clone()
                        }
                    };
                    if to_scan.is_empty() {
                        tracing::debug!(
                            "watcher: {} flagged path(s) already up-to-date; no scan needed",
                            paths.len()
                        );
                        continue;
                    }
                    let path_strs: Vec<String> = to_scan
                        .iter()
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect();
                    match jobs::enqueue_scan_targeted(&state.write, &path_strs, 0, 0).await {
                        Ok(Some(id)) => tracing::info!(
                            "content changed; queued targeted scan job {id} ({} path(s))",
                            to_scan.len()
                        ),
                        Ok(None) => tracing::debug!(
                            "content changed; merged {} path(s) into the pending scan",
                            to_scan.len()
                        ),
                        Err(e) => tracing::error!("watcher: enqueue scan failed: {e:#}"),
                    }
                }
            }
        }
    });

    tracing::info!("watching {} for changes", content.display());
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, CreateKind, DataChange, MetadataKind, RemoveKind};

    #[test]
    fn reads_and_atime_are_not_changes() {
        assert!(!is_content_change(&EventKind::Access(AccessKind::Read)));
        assert!(!is_content_change(&EventKind::Access(AccessKind::Close(
            AccessMode::Read
        ))));
        assert!(is_content_change(&EventKind::Access(AccessKind::Close(
            AccessMode::Write
        ))));
        assert!(!is_content_change(&EventKind::Modify(
            ModifyKind::Metadata(MetadataKind::AccessTime)
        )));
        assert!(is_content_change(&EventKind::Create(CreateKind::File)));
        assert!(is_content_change(&EventKind::Remove(RemoveKind::File)));
        assert!(is_content_change(&EventKind::Modify(ModifyKind::Data(
            DataChange::Content
        ))));
        assert!(is_content_change(&EventKind::Any));
    }

    #[test]
    fn folder_ops_trigger_a_full_scan_but_file_ops_do_not() {
        let folder = Path::new("/lib/books/My Series");
        let archive = Path::new("/lib/books/My Series/v01.cbz");
        let sidecar = Path::new("/lib/books/My Series/ComicInfo.xml");
        let temp = Path::new("/lib/.arca-upload-7.tmp");

        assert!(is_directory_structural_change(
            &EventKind::Remove(RemoveKind::Folder),
            folder
        ));
        assert!(is_directory_structural_change(
            &EventKind::Modify(ModifyKind::Name(notify::event::RenameMode::Any)),
            folder
        ));

        assert!(!is_directory_structural_change(
            &EventKind::Remove(RemoveKind::File),
            archive
        ));
        assert!(!is_directory_structural_change(
            &EventKind::Modify(ModifyKind::Name(notify::event::RenameMode::Any)),
            archive
        ));
        assert!(!is_directory_structural_change(
            &EventKind::Modify(ModifyKind::Name(notify::event::RenameMode::From)),
            temp
        ));
        assert!(!is_directory_structural_change(
            &EventKind::Remove(RemoveKind::File),
            sidecar
        ));
    }
}
