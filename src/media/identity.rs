//! Content-equality schemes and two-tier hashing.
//!
//! Scheme tags version the hash. Structural hashes form non-unique buckets; deep
//! hashes resolve collisions. Classification uses magic bytes, not extensions.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result};

use crate::media::{archive, fingerprint};

/// A content-hashing scheme. `tag()` is stored on the item and is part of the
/// bucket key, so it is also the hash version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    ZipStructuralV1,
    EpubStructuralV1,
    RarStructuralV2,
    /// Sampled byte identity for unrecognized containers.
    BytesV1,
}

impl Scheme {
    pub fn tag(self) -> &'static str {
        match self {
            Scheme::ZipStructuralV1 => "zip-structural-v1",
            Scheme::EpubStructuralV1 => "epub-structural-v1",
            Scheme::RarStructuralV2 => "rar-structural-v2",
            Scheme::BytesV1 => "bytes-v1",
        }
    }

    pub fn from_tag(tag: &str) -> Option<Scheme> {
        match tag {
            "zip-structural-v1" => Some(Scheme::ZipStructuralV1),
            "epub-structural-v1" => Some(Scheme::EpubStructuralV1),
            "rar-structural-v2" => Some(Scheme::RarStructuralV2),
            "bytes-v1" => Some(Scheme::BytesV1),
            _ => None,
        }
    }
}

pub fn classify(magic: &[u8]) -> Scheme {
    if magic.starts_with(b"PK\x03\x04")
        || magic.starts_with(b"PK\x05\x06")
        || magic.starts_with(b"PK\x07\x08")
    {
        Scheme::ZipStructuralV1
    } else {
        Scheme::BytesV1
    }
}

/// The eager identity of one file.
pub enum Identity {
    Ready {
        scheme: Scheme,
        structural_hash: String,
        pages: Option<Vec<String>>,
    },
    NotReady,
}

pub fn identify(path: &Path) -> Result<Identity> {
    let mut file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let size = file.metadata()?.len();
    let mut magic = [0u8; 4];
    let n = read_up_to(&mut file, &mut magic)?;
    if magic[..n].starts_with(b"Rar!") {
        return Ok(match crate::media::rar::inspect(path) {
            Ok(insp) => Identity::Ready {
                scheme: Scheme::RarStructuralV2,
                structural_hash: insp.structural_hash,
                pages: Some(insp.pages),
            },
            Err(_) => Identity::NotReady,
        });
    }
    identify_reader(&mut file, size)
}

pub fn identify_reader<R: Read + Seek>(reader: &mut R, size: u64) -> Result<Identity> {
    let mut magic = [0u8; 4];
    reader.seek(SeekFrom::Start(0))?;
    let n = read_up_to(reader, &mut magic)?;
    reader.seek(SeekFrom::Start(0))?;

    match classify(&magic[..n]) {
        Scheme::ZipStructuralV1 => {
            if !has_eocd_in_tail(reader.by_ref(), size)? {
                return Ok(Identity::NotReady);
            }
            reader.seek(SeekFrom::Start(0))?;
            match archive::inspect_reader(reader.by_ref()) {
                Ok((inspection, _)) if inspection.is_epub => Ok(Identity::Ready {
                    scheme: Scheme::EpubStructuralV1,
                    structural_hash: inspection.epub_hash,
                    pages: None,
                }),
                Ok((inspection, _)) => Ok(Identity::Ready {
                    scheme: Scheme::ZipStructuralV1,
                    structural_hash: inspection.structural_hash,
                    pages: Some(inspection.pages),
                }),
                Err(_) => Ok(Identity::NotReady),
            }
        }
        Scheme::BytesV1 => Ok(Identity::Ready {
            scheme: Scheme::BytesV1,
            structural_hash: fingerprint::of_reader(reader, size)?,
            pages: None,
        }),
        Scheme::RarStructuralV2 | Scheme::EpubStructuralV1 => Ok(Identity::NotReady),
    }
}

pub fn deep_hash(path: &Path, scheme: Scheme) -> Result<String> {
    match scheme {
        Scheme::ZipStructuralV1 | Scheme::EpubStructuralV1 => zip_deep_hash(path),
        Scheme::RarStructuralV2 => crate::media::rar::deep_hash(path),
        Scheme::BytesV1 => byte_deep_hash(path),
    }
}

fn zip_deep_hash(path: &Path) -> Result<String> {
    let mut zip = archive::open_zip(path)?;
    let mut entries: Vec<(String, String)> = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        if !entry.is_file() {
            continue;
        }
        let name = normalize_entry_path(entry.name());
        let mut h = blake3::Hasher::new();
        std::io::copy(&mut entry, &mut HashSink(&mut h))?;
        entries.push((name, h.finalize().to_hex().to_string()));
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

/// Full-file blake3, streamed (memory-flat).
fn byte_deep_hash(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut h = blake3::Hasher::new();
    h.update_reader(&mut file)?;
    Ok(h.finalize().to_hex().to_string())
}

fn normalize_entry_path(name: &str) -> String {
    name.replace('\\', "/").to_ascii_lowercase()
}

/// BLAKE3 writer used to hash decompressed entries without buffering them.
struct HashSink<'a>(&'a mut blake3::Hasher);
impl std::io::Write for HashSink<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.update(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Fill as much of a small probe buffer as the file provides.
fn read_up_to<R: Read>(r: &mut R, buf: &mut [u8]) -> Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

fn has_eocd_in_tail<R: Read + Seek>(reader: &mut R, size: u64) -> Result<bool> {
    const EOCD_SIG: [u8; 4] = [b'P', b'K', 0x05, 0x06];
    const MAX_TAIL: u64 = 22 + 0xFFFF; // EOCD record + max comment length
    let tail = size.min(MAX_TAIL);
    if tail < EOCD_SIG.len() as u64 {
        return Ok(false);
    }
    reader.seek(SeekFrom::End(-(tail as i64)))?;
    let mut buf = vec![0u8; tail as usize];
    let n = read_up_to(reader, &mut buf)?;
    Ok(buf[..n].windows(EOCD_SIG.len()).any(|w| w == EOCD_SIG))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_cbz(path: &Path, pages: &[(&str, &[u8])]) {
        let f = std::fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, data) in pages {
            z.start_file(*name, opts).unwrap();
            z.write_all(data).unwrap();
        }
        z.finish().unwrap();
    }

    #[test]
    fn zip_magic_classifies_as_zip_scheme() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.cbz");
        write_cbz(&p, &[("001.jpg", b"hello")]);
        match identify(&p).unwrap() {
            Identity::Ready { scheme, pages, .. } => {
                assert_eq!(scheme, Scheme::ZipStructuralV1);
                assert_eq!(pages.unwrap(), vec!["001.jpg".to_string()]);
            }
            Identity::NotReady => panic!("a complete zip must be Ready"),
        }
    }

    #[test]
    fn non_zip_classifies_as_bytes_scheme() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("blob.bin");
        std::fs::write(&p, vec![9u8; 5000]).unwrap();
        match identify(&p).unwrap() {
            Identity::Ready { scheme, pages, .. } => {
                assert_eq!(scheme, Scheme::BytesV1);
                assert!(pages.is_none());
            }
            Identity::NotReady => panic!("bytes are always Ready"),
        }
    }

    #[test]
    fn truncated_zip_is_not_ready() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("partial.cbz");
        std::fs::write(&p, b"PK\x03\x04 and then garbage, no central directory").unwrap();
        assert!(matches!(identify(&p).unwrap(), Identity::NotReady));
    }

    #[test]
    fn eocd_tail_check_detects_signature() {
        use std::io::Cursor;
        let mut partial = Cursor::new(b"PK\x03\x04 lots of data but no end record".to_vec());
        let size = partial.get_ref().len() as u64;
        assert!(!has_eocd_in_tail(&mut partial, size).unwrap());

        let mut whole = b"PK\x03\x04 data ".to_vec();
        whole.extend_from_slice(b"PK\x05\x06rest-of-eocd");
        let mut c = Cursor::new(whole.clone());
        assert!(has_eocd_in_tail(&mut c, whole.len() as u64).unwrap());

        let mut tiny = Cursor::new(b"PK".to_vec());
        assert!(!has_eocd_in_tail(&mut tiny, 2).unwrap());
    }

    #[test]
    fn large_partial_zip_is_not_ready_cheaply() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big.cbz");
        let mut data = vec![0u8; 8 * 1024 * 1024];
        let mut x: u64 = 0x9E3779B97F4A7C15;
        for chunk in data.chunks_mut(8) {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            for (i, b) in chunk.iter_mut().enumerate() {
                *b = (x >> (i * 8)) as u8;
            }
        }
        data[0..4].copy_from_slice(b"PK\x03\x04");
        std::fs::write(&p, &data).unwrap();
        assert!(matches!(identify(&p).unwrap(), Identity::NotReady));
    }

    #[test]
    fn structural_hash_is_container_independent() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.cbz");
        let b = dir.path().join("b.cbz");
        write_cbz(&a, &[("001.jpg", b"page-one"), ("002.jpg", b"page-two")]);
        write_cbz(&b, &[("002.jpg", b"page-two"), ("001.jpg", b"page-one")]);
        let ha = structural_of(&a);
        let hb = structural_of(&b);
        assert_eq!(ha, hb, "reordered re-zip must share a structural hash");

        let c = dir.path().join("c.cbz");
        write_cbz(&c, &[("001.jpg", b"page-one"), ("002.jpg", b"CHANGED!")]);
        assert_ne!(ha, structural_of(&c));
    }

    #[test]
    fn deep_hash_separates_a_structural_hash_tie() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.cbz");
        let b = dir.path().join("b.cbz");
        write_cbz(&a, &[("001.jpg", b"same"), ("002.jpg", b"pages")]);
        write_cbz(&b, &[("002.jpg", b"pages"), ("001.jpg", b"same")]);
        assert_eq!(
            deep_hash(&a, Scheme::ZipStructuralV1).unwrap(),
            deep_hash(&b, Scheme::ZipStructuralV1).unwrap()
        );
    }

    fn structural_of(p: &Path) -> String {
        match identify(p).unwrap() {
            Identity::Ready {
                structural_hash, ..
            } => structural_hash,
            Identity::NotReady => panic!("ready"),
        }
    }

    fn write_text_epub(path: &Path, body: &[u8]) {
        write_cbz(
            path,
            &[
                (
                    "META-INF/container.xml",
                    br#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
                ),
                (
                    "content.opf",
                    br#"<package xmlns="http://www.idpf.org/2007/opf"><manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#,
                ),
                ("c1.xhtml", body),
            ],
        );
    }

    #[test]
    fn epub_gets_its_own_scheme() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("book.epub");
        write_text_epub(&p, b"<html><body>a story</body></html>");
        match identify(&p).unwrap() {
            Identity::Ready { scheme, pages, .. } => {
                assert_eq!(scheme, Scheme::EpubStructuralV1);
                assert!(pages.is_none(), "reflowable: no image page list");
            }
            Identity::NotReady => panic!("a complete epub must be Ready"),
        }
    }

    #[test]
    fn distinct_text_epubs_do_not_false_merge() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.epub");
        let b = dir.path().join("b.epub");
        write_text_epub(&a, b"<html><body>the first book</body></html>");
        write_text_epub(&b, b"<html><body>a completely different book</body></html>");
        assert_ne!(
            structural_of(&a),
            structural_of(&b),
            "different text books must not share a structural hash"
        );

        let c = dir.path().join("c.cbz");
        write_cbz(&c, &[("001.jpg", b"page")]);
        match identify(&c).unwrap() {
            Identity::Ready { scheme, .. } => assert_eq!(scheme, Scheme::ZipStructuralV1),
            Identity::NotReady => panic!("ready"),
        }
    }
}
