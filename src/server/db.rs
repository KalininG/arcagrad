//! SQLite pools and migrations.

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;

#[derive(Clone)]
pub struct Pools {
    pub read: SqlitePool,
    pub write: SqlitePool,
}

/// Reject network filesystems because SQLite WAL requires local locking.
/// `ARCA_ALLOW_NETWORK_DATA_DIR=1` overrides the check.
pub fn guard_data_dir_local(data_dir: &Path) -> anyhow::Result<()> {
    let Some(fs) = network_fs_name(data_dir) else {
        return Ok(());
    };
    let allow = std::env::var("ARCA_ALLOW_NETWORK_DATA_DIR")
        .ok()
        .and_then(|v| crate::server::config::parse_bool(&v))
        .unwrap_or(false);
    if allow {
        tracing::warn!(
            "ARCA_DATA_DIR ({}) is on a {fs} filesystem — SQLite WAL locking is \
             unsafe on network filesystems and can CORRUPT the database. Proceeding \
             because ARCA_ALLOW_NETWORK_DATA_DIR is set; you are on your own.",
            data_dir.display()
        );
        return Ok(());
    }
    anyhow::bail!(
        "ARCA_DATA_DIR ({}) is on a {fs} filesystem. SQLite's write-ahead-log \
         locking is unsafe on network filesystems (NFS/SMB/9p/FUSE/cluster FS) and \
         WILL eventually corrupt the database. Put ARCA_DATA_DIR on LOCAL disk (the \
         library ARCA_CONTENT_DIR on a NAS mount is fine — only the database must be \
         local). If you are certain this mount is safe, set \
         ARCA_ALLOW_NETWORK_DATA_DIR=1 to override.",
        data_dir.display()
    );
}

/// Return the Linux network-filesystem type for `path`.
#[cfg(target_os = "linux")]
fn network_fs_name(path: &Path) -> Option<&'static str> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut s: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut s) } != 0 {
        return None;
    }
    match s.f_type as i64 {
        0x6969 => Some("NFS"),
        0x517B | 0xFF534D42 => Some("SMB/CIFS"),
        0xFE534D42 => Some("SMB2"),
        0x0102_1997 => Some("9p (Plan 9 / VirtFS)"),
        0x6573_5546 => Some("FUSE"),
        0x7461_636F => Some("OCFS2 (cluster)"),
        0x0116_1970 => Some("GFS2 (cluster)"),
        0x0BD0_0BD0 => Some("Lustre"),
        0x00C3_6400 => Some("Ceph"),
        _ => None,
    }
}

#[cfg(not(target_os = "linux"))]
fn network_fs_name(_path: &Path) -> Option<&'static str> {
    None
}

/// Open the database, run migrations, and return its pools.
pub async fn connect(data_dir: &Path) -> anyhow::Result<Pools> {
    tokio::fs::create_dir_all(data_dir)
        .await
        .with_context(|| format!("create data dir {}", data_dir.display()))?;

    let db_path = data_dir.join("arca.db");

    // Serialize writes through one connection.
    let write_opts = SqliteConnectOptions::new()
        .filename(&db_path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5))
        .foreign_keys(true);
    let write = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(write_opts)
        .await
        .with_context(|| format!("open sqlite writer {}", db_path.display()))?;

    sqlx::migrate!("./migrations")
        .run(&write)
        .await
        .context("run migrations")?;

    // Readers remain read-only and may run concurrently under WAL.
    let read_opts = SqliteConnectOptions::new()
        .filename(&db_path)
        .read_only(true)
        .busy_timeout(Duration::from_secs(5))
        .foreign_keys(true);
    let read = SqlitePoolOptions::new()
        .max_connections(16)
        .connect_with(read_opts)
        .await
        .with_context(|| format!("open sqlite readers {}", db_path.display()))?;

    Ok(Pools { read, write })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_temp_dir_is_not_flagged_as_network_fs() {
        let dir = tempfile::tempdir().unwrap();
        assert!(network_fs_name(dir.path()).is_none());
        assert!(guard_data_dir_local(dir.path()).is_ok());
    }

    #[test]
    fn unstattable_path_does_not_block_boot() {
        let p = std::path::Path::new("/nonexistent/arca/data/dir/xyz");
        assert!(network_fs_name(p).is_none());
    }
}
