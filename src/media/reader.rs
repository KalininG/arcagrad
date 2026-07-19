//! Magic-byte dispatch for archive page listing and reading.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use anyhow::Result;

use crate::server::stat_lru::StatValidatedLru;

fn is_rar(path: &Path) -> Result<bool> {
    let mut magic = [0u8; 4];
    let n = std::fs::File::open(path)?.read(&mut magic)?;
    Ok(magic[..n].starts_with(b"Rar!"))
}

/// The reading-order page (entry) names for an archive, whatever its container.
pub fn list_pages(path: &Path) -> Result<Vec<String>> {
    if is_rar(path)? {
        Ok(crate::media::rar::inspect(path)?.pages)
    } else {
        crate::media::archive::list_pages(path)
    }
}

pub fn read_entry(path: &Path, name: &str) -> Result<(Vec<u8>, &'static str)> {
    match get_or_open(path)? {
        Handle::Rar => crate::media::rar::read_page(path, name),
        Handle::Zip(index) => index.read_entry(name),
    }
}

const MAX_ARCHIVE_HANDLES: usize = 32;

#[derive(Clone)]
enum Handle {
    Zip(Arc<crate::media::archive::ZipIndex>),
    Rar,
}

static HANDLES: LazyLock<StatValidatedLru<PathBuf, Handle>> =
    LazyLock::new(|| StatValidatedLru::new(MAX_ARCHIVE_HANDLES));

fn get_or_open(path: &Path) -> Result<Handle> {
    let meta = std::fs::metadata(path)?;
    let (len, mtime) = (meta.len(), meta.modified().ok());
    if let Some(handle) = HANDLES.get_validated(path, len, mtime) {
        return Ok(handle);
    }
    let handle = if is_rar(path)? {
        Handle::Rar
    } else {
        Handle::Zip(Arc::new(crate::media::archive::ZipIndex::open(path)?))
    };
    HANDLES.put(path.to_path_buf(), handle.clone(), len, mtime);
    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let f = std::fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            z.start_file(*name, opts).unwrap();
            z.write_all(data).unwrap();
        }
        z.finish().unwrap();
    }

    #[test]
    fn dispatches_zip_and_rar_by_magic_not_extension() {
        let dir = tempfile::tempdir().unwrap();

        let zp = dir.path().join("z.cbz");
        write_zip(&zp, &[("001.jpg", b"ZIP-PAGE")]);
        assert_eq!(list_pages(&zp).unwrap(), vec!["001.jpg".to_string()]);
        assert_eq!(read_entry(&zp, "001.jpg").unwrap().0, b"ZIP-PAGE");

        let rp = dir.path().join("mislabeled.cbz");
        std::fs::write(&rp, crate::media::rar::rar4(&[("001.jpg", b"RAR-PAGE")])).unwrap();
        assert_eq!(list_pages(&rp).unwrap(), vec!["001.jpg".to_string()]);
        let (bytes, ct) = read_entry(&rp, "001.jpg").unwrap();
        assert_eq!(bytes, b"RAR-PAGE");
        assert_eq!(ct, "image/jpeg");
    }

    #[test]
    fn pooled_handle_reused_then_invalidated_when_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.cbz");

        write_zip(&p, &[("001.jpg", b"V1")]);
        assert_eq!(read_entry(&p, "001.jpg").unwrap().0, b"V1");
        assert_eq!(read_entry(&p, "001.jpg").unwrap().0, b"V1");

        std::thread::sleep(std::time::Duration::from_millis(20));
        write_zip(&p, &[("001.jpg", b"VERSION-TWO")]);
        assert_eq!(read_entry(&p, "001.jpg").unwrap().0, b"VERSION-TWO");
    }
}
