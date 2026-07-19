//! Decode-only RAR/CBR inspection, reading, and identity hashing.

use std::path::Path;

use anyhow::{anyhow, Result};
use unrar::Archive;

/// Page names + a `structural_hash`, from one header pass (no page decompression).
/// The RAR counterpart of [`archive::Inspection`](crate::media::archive::Inspection).
pub struct Inspection {
    pub pages: Vec<String>,
    pub structural_hash: String,
}

pub fn inspect(path: &Path) -> Result<Inspection> {
    let mut tuples: Vec<(String, u64, u32)> = Vec::new();
    let listing = Archive::new(&path)
        .open_for_listing()
        .map_err(|e| anyhow!("open rar for listing: {e}"))?;
    for entry in listing {
        let e = entry.map_err(|e| anyhow!("read rar header: {e}"))?;
        if e.is_file() {
            let name = e.filename.to_string_lossy().into_owned();
            tuples.push((name, e.unpacked_size, e.file_crc));
        }
    }
    if tuples.is_empty() {
        return Err(anyhow!("rar has no file entries"));
    }
    tuples.sort_by(|a, b| crate::media::series::natural_cmp(&a.0, &b.0).then_with(|| a.cmp(b)));
    let mut h = blake3::Hasher::new();
    for (name, size, crc) in &tuples {
        h.update(name.as_bytes());
        h.update(&[0]);
        h.update(&size.to_le_bytes());
        h.update(&crc.to_le_bytes());
    }
    let pages = tuples.into_iter().map(|(n, _, _)| n).collect();
    Ok(Inspection {
        pages,
        structural_hash: h.finalize().to_hex().to_string(),
    })
}

pub fn read_page(path: &Path, name: &str) -> Result<(Vec<u8>, &'static str)> {
    let mut archive = Archive::new(&path)
        .open_for_processing()
        .map_err(|e| anyhow!("open rar for processing: {e}"))?;
    loop {
        let Some(header) = archive
            .read_header()
            .map_err(|e| anyhow!("read rar header: {e}"))?
        else {
            break;
        };
        let is_match =
            header.entry().is_file() && header.entry().filename.to_string_lossy() == *name;
        if is_match {
            let (bytes, _next) = header
                .read()
                .map_err(|e| anyhow!("extract rar entry: {e}"))?;
            return Ok((bytes, crate::media::archive::content_type_for(name)));
        }
        archive = header.skip().map_err(|e| anyhow!("skip rar entry: {e}"))?;
    }
    Err(anyhow!("entry not found in rar: {name}"))
}

pub fn deep_hash(path: &Path) -> Result<String> {
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut archive = Archive::new(&path)
        .open_for_processing()
        .map_err(|e| anyhow!("open rar for processing: {e}"))?;
    loop {
        let Some(header) = archive
            .read_header()
            .map_err(|e| anyhow!("read rar header: {e}"))?
        else {
            break;
        };
        if header.entry().is_file() {
            let name = header.entry().filename.to_string_lossy().into_owned();
            let (bytes, next) = header
                .read()
                .map_err(|e| anyhow!("extract rar entry: {e}"))?;
            entries.push((name, blake3::hash(&bytes).to_hex().to_string()));
            archive = next;
        } else {
            archive = header.skip().map_err(|e| anyhow!("skip rar entry: {e}"))?;
        }
    }
    entries.sort();
    let mut h = blake3::Hasher::new();
    for (name, entry_hash) in &entries {
        h.update(name.as_bytes());
        h.update(&[0]);
        h.update(entry_hash.as_bytes());
        h.update(&[0]);
    }
    Ok(h.finalize().to_hex().to_string())
}

/// Standard CRC-32 (poly 0xEDB88320) for the RAR4 test fixture builder.
#[cfg(test)]
fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
pub(crate) fn rar4(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"Rar!\x1a\x07\x00");

    let mut main = vec![0x73u8];
    main.extend_from_slice(&0u16.to_le_bytes());
    main.extend_from_slice(&13u16.to_le_bytes());
    main.extend_from_slice(&[0u8; 6]);
    out.extend_from_slice(&((crc32(&main) & 0xffff) as u16).to_le_bytes());
    out.extend_from_slice(&main);

    for (name, data) in entries {
        let name_b = name.as_bytes();
        let mut fh = vec![0x74u8];
        fh.extend_from_slice(&0x8000u16.to_le_bytes());
        fh.extend_from_slice(&(32u16 + name_b.len() as u16).to_le_bytes());
        fh.extend_from_slice(&(data.len() as u32).to_le_bytes());
        fh.extend_from_slice(&(data.len() as u32).to_le_bytes());
        fh.push(0x03);
        fh.extend_from_slice(&crc32(data).to_le_bytes());
        fh.extend_from_slice(&0u32.to_le_bytes());
        fh.push(0x14);
        fh.push(0x30);
        fh.extend_from_slice(&(name_b.len() as u16).to_le_bytes());
        fh.extend_from_slice(&0u32.to_le_bytes());
        fh.extend_from_slice(name_b);
        out.extend_from_slice(&((crc32(&fh) & 0xffff) as u16).to_le_bytes());
        out.extend_from_slice(&fh);
        out.extend_from_slice(data);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_rar(
        dir: &std::path::Path,
        name: &str,
        entries: &[(&str, &[u8])],
    ) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, rar4(entries)).unwrap();
        p
    }

    #[test]
    fn inspect_lists_pages_and_hashes_from_headers() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_rar(
            dir.path(),
            "a.cbr",
            &[("002.jpg", b"page-two"), ("001.jpg", b"page-one")],
        );
        let insp = inspect(&p).unwrap();
        assert_eq!(
            insp.pages,
            vec!["001.jpg".to_string(), "002.jpg".to_string()]
        );
        assert_eq!(insp.structural_hash.len(), 64);
        assert_eq!(insp.structural_hash, inspect(&p).unwrap().structural_hash);
    }

    #[test]
    fn unpadded_numeric_pages_sort_naturally_not_lexically() {
        let dir = tempfile::tempdir().unwrap();
        let names: Vec<String> = (1..=20).map(|n| format!("{n}.jpg")).collect();
        let entries: Vec<(&str, &[u8])> =
            names.iter().map(|n| (n.as_str(), b"x".as_ref())).collect();
        let p = write_rar(dir.path(), "a.cbr", &entries);
        assert_eq!(inspect(&p).unwrap().pages, names);
    }

    #[test]
    fn structural_hash_is_entry_order_independent() {
        let dir = tempfile::tempdir().unwrap();
        let a = write_rar(dir.path(), "a.cbr", &[("1.jpg", b"one"), ("2.jpg", b"two")]);
        let b = write_rar(dir.path(), "b.cbr", &[("2.jpg", b"two"), ("1.jpg", b"one")]);
        assert_eq!(
            inspect(&a).unwrap().structural_hash,
            inspect(&b).unwrap().structural_hash
        );
        let c = write_rar(
            dir.path(),
            "c.cbr",
            &[("1.jpg", b"one"), ("2.jpg", b"TWO!")],
        );
        assert_ne!(
            inspect(&a).unwrap().structural_hash,
            inspect(&c).unwrap().structural_hash
        );
    }

    #[test]
    fn read_page_returns_exact_bytes_and_content_type() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_rar(
            dir.path(),
            "a.cbr",
            &[("001.jpg", b"JPEG-BYTES-1"), ("002.png", b"PNG-BYTES-2")],
        );
        let (bytes, ct) = read_page(&p, "002.png").unwrap();
        assert_eq!(bytes, b"PNG-BYTES-2");
        assert_eq!(ct, "image/png");
        let (bytes, ct) = read_page(&p, "001.jpg").unwrap();
        assert_eq!(bytes, b"JPEG-BYTES-1");
        assert_eq!(ct, "image/jpeg");
        assert!(read_page(&p, "nope.jpg").is_err());
    }

    #[test]
    fn deep_hash_reflects_content_not_order() {
        let dir = tempfile::tempdir().unwrap();
        let a = write_rar(dir.path(), "a.cbr", &[("1", b"x"), ("2", b"y")]);
        let b = write_rar(dir.path(), "b.cbr", &[("2", b"y"), ("1", b"x")]);
        assert_eq!(deep_hash(&a).unwrap(), deep_hash(&b).unwrap());
        let c = write_rar(dir.path(), "c.cbr", &[("1", b"x"), ("2", b"Y")]);
        assert_ne!(deep_hash(&a).unwrap(), deep_hash(&c).unwrap());
    }

    #[test]
    fn inspect_errors_on_a_non_rar_or_truncated_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("bad.cbr");
        std::fs::write(&p, b"Rar!\x1a\x07\x00 and then nonsense").unwrap();
        assert!(inspect(&p).is_err());
    }

    #[test]
    #[ignore = "reads a local CBR via ARCA_RAR_TEST_FILE; run with --ignored to validate the RAR reader"]
    fn inspects_a_real_cbr() {
        let configured_path = std::env::var_os("ARCA_RAR_TEST_FILE")
            .expect("set ARCA_RAR_TEST_FILE to a local CBR before running this ignored test");
        let path = Path::new(&configured_path);
        let insp = inspect(path).expect("inspect");
        eprintln!(
            "{} pages; first {:?}; structural={}",
            insp.pages.len(),
            &insp.pages[..insp.pages.len().min(3)],
            insp.structural_hash
        );
        assert!(!insp.pages.is_empty());
        assert_eq!(insp.structural_hash.len(), 64, "blake3 hex");
        assert_eq!(insp.structural_hash, inspect(path).unwrap().structural_hash);

        let deep = deep_hash(path).expect("deep_hash");
        eprintln!("deep={deep}");
        assert_eq!(deep.len(), 64);

        let name = &insp.pages[insp.pages.len() / 2];
        let (bytes, ct) = read_page(path, name).expect("read_page");
        eprintln!("read_page({name}) = {} bytes, {ct}", bytes.len());
        assert!(!bytes.is_empty());
        let looks_image = bytes.starts_with(b"\xff\xd8\xff")
            || bytes.starts_with(b"\x89PNG")
            || (bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP");
        assert!(looks_image, "a page must decode to image bytes");
    }
}
