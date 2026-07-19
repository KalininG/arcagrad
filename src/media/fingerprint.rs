//! Sampled BLAKE3 fingerprint for the `bytes-v1` identity scheme.
//!
//! The input is `size_le || first 1 MiB || last 1 MiB`; small files are hashed whole.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::Result;

pub const HEAD: u64 = 1024 * 1024;
pub const TAIL: u64 = 1024 * 1024;

/// Fingerprint an in-memory buffer.
pub fn of_bytes(bytes: &[u8]) -> String {
    let size = bytes.len() as u64;
    let mut h = blake3::Hasher::new();
    h.update(&size.to_le_bytes());
    if size <= HEAD + TAIL {
        h.update(bytes);
    } else {
        h.update(&bytes[..HEAD as usize]);
        h.update(&bytes[(size - TAIL) as usize..]);
    }
    h.finalize().to_hex().to_string()
}

pub fn of_reader<R: Read + Seek>(reader: &mut R, size: u64) -> Result<String> {
    let mut h = blake3::Hasher::new();
    h.update(&size.to_le_bytes());
    if size <= HEAD + TAIL {
        reader.seek(SeekFrom::Start(0))?;
        h.update_reader(reader.by_ref().take(size))?;
    } else {
        reader.seek(SeekFrom::Start(0))?;
        h.update_reader(reader.by_ref().take(HEAD))?;
        reader.seek(SeekFrom::Start(size - TAIL))?;
        h.update_reader(reader.by_ref().take(TAIL))?;
    }
    Ok(h.finalize().to_hex().to_string())
}

/// Fingerprint a file using its size and head/tail windows.
pub fn of_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let size = f.metadata()?.len();
    of_reader(&mut f, size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    #[test]
    fn reader_bytes_and_file_forms_agree() {
        for size in [
            0usize,
            100,
            (HEAD + TAIL) as usize,
            (HEAD + TAIL) as usize + 1,
            40 * 1024 * 1024,
        ] {
            let data: Vec<u8> = (0..size).map(|i| (i * 2654435761usize) as u8).collect();
            let by_bytes = of_bytes(&data);
            let by_reader = of_reader(&mut Cursor::new(&data), size as u64).unwrap();
            let dir = tempfile::tempdir().unwrap();
            let p = dir.path().join("f.bin");
            std::fs::File::create(&p).unwrap().write_all(&data).unwrap();
            let by_file = of_file(&p).unwrap();
            assert_eq!(by_bytes, by_reader, "bytes vs reader @ {size}");
            assert_eq!(by_bytes, by_file, "bytes vs file @ {size}");
        }
    }

    #[test]
    fn middle_only_change_is_the_documented_blind_spot() {
        let n = (HEAD + TAIL) as usize + 4 * 1024 * 1024;
        let mut a = vec![7u8; n];
        let mut b = a.clone();
        let mid = n / 2;
        a[mid] = 1;
        b[mid] = 2;
        assert_eq!(
            of_bytes(&a),
            of_bytes(&b),
            "mid-only change collides (known)"
        );
        b[0] = 9;
        assert_ne!(of_bytes(&a), of_bytes(&b));
    }

    fn build_cbz(pages: &[(&str, &[u8])]) -> Vec<u8> {
        let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .last_modified_time(zip::DateTime::default());
        for (name, data) in pages {
            z.start_file(*name, opts).unwrap();
            z.write_all(data).unwrap();
        }
        z.finish().unwrap().into_inner()
    }

    #[test]
    fn zip_central_directory_catches_same_size_middle_page_edit() {
        let big1 = vec![0xAAu8; 1_500_000];
        let big3 = vec![0xCCu8; 1_500_000];
        let mut mid = vec![0xBBu8; 1_000_000];
        let a = build_cbz(&[("001.jpg", &big1), ("002.jpg", &mid), ("003.jpg", &big3)]);
        mid[500_000] ^= 0x01;
        let b = build_cbz(&[("001.jpg", &big1), ("002.jpg", &mid), ("003.jpg", &big3)]);

        assert!(
            a.len() as u64 > HEAD + TAIL,
            "archive must exceed the windows so page 2 is in the middle gap"
        );
        assert_eq!(
            a.len(),
            b.len(),
            "identical total size — isolates the central-directory mechanism, not size"
        );
        assert_ne!(
            of_bytes(&a),
            of_bytes(&b),
            "a middle-page edit still changes the fingerprint via the central directory"
        );
    }

    #[test]
    fn similar_variants_all_differ() {
        let p1 = vec![1u8; 1_200_000];
        let p2 = vec![2u8; 1_200_000];
        let base = build_cbz(&[("001.jpg", &p1), ("002.jpg", &p2)]);

        let p2_edited = vec![2u8; 1_200_050];
        let resized = build_cbz(&[("001.jpg", &p1), ("002.jpg", &p2_edited)]);
        assert_ne!(
            of_bytes(&base),
            of_bytes(&resized),
            "size-changing page edit"
        );

        let p1b = vec![9u8; 1_200_000];
        let p2b = vec![8u8; 1_200_000];
        let reencoded = build_cbz(&[("001.jpg", &p1b), ("002.jpg", &p2b)]);
        assert_ne!(of_bytes(&base), of_bytes(&reencoded), "re-encode");

        let p3 = vec![3u8; 1_200_000];
        let extra = build_cbz(&[("001.jpg", &p1), ("002.jpg", &p2), ("003.jpg", &p3)]);
        assert_ne!(of_bytes(&base), of_bytes(&extra), "different page count");

        let same = build_cbz(&[("001.jpg", &p1), ("002.jpg", &p2)]);
        assert_eq!(
            of_bytes(&base),
            of_bytes(&same),
            "identical content → identical id"
        );
    }
}
