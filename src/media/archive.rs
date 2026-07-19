//! ZIP/CBZ inspection and random-access reading.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "avif", "bmp"];

pub(crate) fn open_zip(path: &Path) -> Result<zip::ZipArchive<File>> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    zip::ZipArchive::new(file).context("read zip central directory")
}

pub fn list_pages(path: &Path) -> Result<Vec<String>> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let size = file.metadata()?.len();
    let mut names: Vec<String> = match central_directory_names(&mut file, size) {
        Ok(names) => names.into_iter().filter(|n| is_image(n)).collect(),
        Err(e) => {
            tracing::debug!(
                "central-directory fast parse failed for {}: {e}; using zip crate",
                path.display()
            );
            open_zip(path)?
                .file_names()
                .filter(|n| is_image(n))
                .map(str::to_string)
                .collect()
        }
    };
    names.sort_by(|a, b| crate::media::series::natural_cmp(a, b));
    Ok(names)
}

#[derive(Clone)]
struct CdRecord {
    name: String,
    flags: u16,
    compression: u16,
    crc32: u32,
    compressed_size: u64,
    uncompressed_size: u64,
    local_header_offset: u64,
    /// `version made by` (CD offset 4): its high byte is the host system, needed to read
    /// `external_attributes` as a unix mode for the exact `is_file` (symlink) test.
    version_made_by: u16,
    /// External file attributes (CD offset 38): carries the unix mode (`>> 16`) / DOS bits.
    external_attributes: u32,
}

fn central_directory<R: Read + Seek>(file: &mut R, size: u64) -> Result<Vec<CdRecord>> {
    use anyhow::bail;
    const EOCD_SIG: u32 = 0x0605_4b50;
    const EOCD64_LOC_SIG: u32 = 0x0706_4b50;
    const EOCD64_SIG: u32 = 0x0606_4b50;
    const CEN_SIG: u32 = 0x0201_4b50;
    let le16 = |b: &[u8]| u16::from_le_bytes(b[..2].try_into().unwrap());
    let le32 = |b: &[u8]| u32::from_le_bytes(b[..4].try_into().unwrap());
    let le64 = |b: &[u8]| u64::from_le_bytes(b[..8].try_into().unwrap());

    if size < 22 {
        bail!("file too small for a zip EOCD");
    }
    let tail_len = size.min(22 + 0xFFFF);
    file.seek(SeekFrom::Start(size - tail_len))?;
    let mut tail = vec![0u8; tail_len as usize];
    file.read_exact(&mut tail)?;
    let eocd = (0..=tail.len() - 22)
        .rev()
        .find(|&i| {
            le32(&tail[i..]) == EOCD_SIG && i + 22 + le16(&tail[i + 20..]) as usize == tail.len()
        })
        .ok_or_else(|| anyhow::anyhow!("no EOCD record in tail"))?;
    let mut cd_size = le32(&tail[eocd + 12..]) as u64;
    let mut cd_offset = le32(&tail[eocd + 16..]) as u64;

    if cd_offset == 0xFFFF_FFFF || cd_size == 0xFFFF_FFFF {
        let eocd_abs = size - tail_len + eocd as u64;
        if eocd_abs < 20 {
            bail!("zip64 locator would precede start of file");
        }
        file.seek(SeekFrom::Start(eocd_abs - 20))?;
        let mut loc = [0u8; 20];
        file.read_exact(&mut loc)?;
        if le32(&loc) != EOCD64_LOC_SIG {
            bail!("expected zip64 EOCD locator");
        }
        let eocd64_off = le64(&loc[8..]);
        file.seek(SeekFrom::Start(eocd64_off))?;
        let mut z = [0u8; 56];
        file.read_exact(&mut z)?;
        if le32(&z) != EOCD64_SIG {
            bail!("expected zip64 EOCD record");
        }
        cd_size = le64(&z[40..]);
        cd_offset = le64(&z[48..]);
    }

    if cd_offset + cd_size > size {
        bail!("central directory extends past end of file");
    }
    file.seek(SeekFrom::Start(cd_offset))?;
    let mut cd = vec![0u8; cd_size as usize];
    file.read_exact(&mut cd)?;

    let mut records = Vec::new();
    let mut p = 0usize;
    while p + 46 <= cd.len() {
        if le32(&cd[p..]) != CEN_SIG {
            break; // end of records (or padding), stop cleanly
        }
        let version_made_by = le16(&cd[p + 4..]);
        let flags = le16(&cd[p + 8..]);
        let compression = le16(&cd[p + 10..]);
        let crc32 = le32(&cd[p + 16..]);
        let mut compressed_size = le32(&cd[p + 20..]) as u64;
        let mut uncompressed_size = le32(&cd[p + 24..]) as u64;
        let n = le16(&cd[p + 28..]) as usize;
        let m = le16(&cd[p + 30..]) as usize;
        let k = le16(&cd[p + 32..]) as usize;
        let external_attributes = le32(&cd[p + 38..]);
        let mut local_header_offset = le32(&cd[p + 42..]) as u64;
        let name_start = p + 46;
        let name_end = name_start + n;
        let extra_end = name_end + m;
        if extra_end > cd.len() {
            break;
        }
        let name = String::from_utf8_lossy(&cd[name_start..name_end]).into_owned();
        if uncompressed_size == 0xFFFF_FFFF
            || compressed_size == 0xFFFF_FFFF
            || local_header_offset == 0xFFFF_FFFF
        {
            let extra = &cd[name_end..extra_end];
            let mut q = 0usize;
            while q + 4 <= extra.len() {
                let id = le16(&extra[q..]);
                let sz = le16(&extra[q + 2..]) as usize;
                let data = &extra[(q + 4).min(extra.len())..(q + 4 + sz).min(extra.len())];
                if id == 0x0001 {
                    let mut r = 0usize;
                    for slot in [
                        &mut uncompressed_size,
                        &mut compressed_size,
                        &mut local_header_offset,
                    ] {
                        if *slot == 0xFFFF_FFFF && r + 8 <= data.len() {
                            *slot = le64(&data[r..]);
                            r += 8;
                        }
                    }
                    break;
                }
                q += 4 + sz;
            }
        }
        records.push(CdRecord {
            name,
            flags,
            compression,
            crc32,
            compressed_size,
            uncompressed_size,
            local_header_offset,
            version_made_by,
            external_attributes,
        });
        p = extra_end + k;
    }
    Ok(records)
}

fn central_directory_names<R: Read + Seek>(file: &mut R, size: u64) -> Result<Vec<String>> {
    Ok(central_directory(file, size)?
        .into_iter()
        .map(|r| r.name)
        .collect())
}

pub struct Inspection {
    pub pages: Vec<String>,
    pub structural_hash: String,
    pub is_epub: bool,
    pub epub_hash: String,
}

pub fn inspect(path: &Path) -> Result<Inspection> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    Ok(inspect_reader(file)?.0)
}

pub fn inspect_reader<R: Read + Seek>(mut reader: R) -> Result<(Inspection, R)> {
    let size = reader.seek(SeekFrom::End(0))?;
    reader.seek(SeekFrom::Start(0))?;
    match central_directory(&mut reader, size) {
        Ok(records) => {
            let inspection = inspection_from_records(&records);
            reader.seek(SeekFrom::Start(0))?;
            Ok((inspection, reader))
        }
        Err(_) => inspect_reader_via_crate(reader),
    }
}

fn inspection_from_records(records: &[CdRecord]) -> Inspection {
    let mut names = Vec::new();
    let mut fingerprints: Vec<(u32, u64)> = Vec::new();
    let mut all_entries: Vec<(String, u32, u64)> = Vec::new();
    let mut is_epub = false;
    for rec in records {
        if !cd_is_file(rec) {
            continue;
        }
        if rec.name.eq_ignore_ascii_case("META-INF/container.xml") {
            is_epub = true;
        }
        all_entries.push((rec.name.clone(), rec.crc32, rec.uncompressed_size));
        if is_image(&rec.name) {
            names.push(rec.name.clone());
            fingerprints.push((rec.crc32, rec.uncompressed_size));
        }
    }
    names.sort_by(|a, b| crate::media::series::natural_cmp(a, b));
    fingerprints.sort_unstable();
    all_entries.sort_unstable();

    let mut hasher = blake3::Hasher::new();
    for (crc, size) in &fingerprints {
        hasher.update(&crc.to_le_bytes());
        hasher.update(&size.to_le_bytes());
    }
    let mut epub_hasher = blake3::Hasher::new();
    for (name, crc, size) in &all_entries {
        epub_hasher.update(name.as_bytes());
        epub_hasher.update(&[0]);
        epub_hasher.update(&crc.to_le_bytes());
        epub_hasher.update(&size.to_le_bytes());
    }
    Inspection {
        pages: names,
        structural_hash: hasher.finalize().to_hex().to_string(),
        is_epub,
        epub_hash: epub_hasher.finalize().to_hex().to_string(),
    }
}

fn cd_is_file(rec: &CdRecord) -> bool {
    const S_IFLNK: u32 = 0o120000;
    let is_dir = rec
        .name
        .chars()
        .next_back()
        .is_some_and(|c| c == '/' || c == '\\');
    let is_symlink = cd_unix_mode(rec).is_some_and(|m| m & S_IFLNK == S_IFLNK);
    !is_dir && !is_symlink
}

fn cd_unix_mode(rec: &CdRecord) -> Option<u32> {
    const S_IFDIR: u32 = 0o040000;
    const S_IFREG: u32 = 0o100000;
    if rec.external_attributes == 0 {
        return None;
    }
    match (rec.version_made_by >> 8) as u8 {
        3 => Some(rec.external_attributes >> 16), // Unix
        0 => {
            let mut mode = if rec.external_attributes & 0x10 == 0x10 {
                S_IFDIR | 0o0775
            } else {
                S_IFREG | 0o0664
            };
            if rec.external_attributes & 0x01 == 0x01 {
                mode &= 0o0555;
            }
            Some(mode)
        }
        _ => None,
    }
}

/// Crate-based fallback for central-directory inspection.
fn inspect_reader_via_crate<R: Read + Seek>(reader: R) -> Result<(Inspection, R)> {
    let mut zip = zip::ZipArchive::new(reader).context("read zip central directory")?;

    let mut names = Vec::new();
    let mut fingerprints: Vec<(u32, u64)> = Vec::new();
    let mut all_entries: Vec<(String, u32, u64)> = Vec::new();
    let mut is_epub = false;
    for i in 0..zip.len() {
        let entry = zip.by_index(i)?;
        if !entry.is_file() {
            continue;
        }
        let name = entry.name();
        if name.eq_ignore_ascii_case("META-INF/container.xml") {
            is_epub = true;
        }
        all_entries.push((name.to_string(), entry.crc32(), entry.size()));
        if is_image(name) {
            names.push(name.to_string());
            fingerprints.push((entry.crc32(), entry.size()));
        }
    }
    names.sort_by(|a, b| crate::media::series::natural_cmp(a, b));
    fingerprints.sort_unstable();
    all_entries.sort_unstable();

    let mut hasher = blake3::Hasher::new();
    for (crc, size) in &fingerprints {
        hasher.update(&crc.to_le_bytes());
        hasher.update(&size.to_le_bytes());
    }
    let mut epub_hasher = blake3::Hasher::new();
    for (name, crc, size) in &all_entries {
        epub_hasher.update(name.as_bytes());
        epub_hasher.update(&[0]);
        epub_hasher.update(&crc.to_le_bytes());
        epub_hasher.update(&size.to_le_bytes());
    }
    let mut reader = zip.into_inner();
    reader.seek(SeekFrom::Start(0))?;
    Ok((
        Inspection {
            pages: names,
            structural_hash: hasher.finalize().to_hex().to_string(),
            is_epub,
            epub_hash: epub_hasher.finalize().to_hex().to_string(),
        },
        reader,
    ))
}

pub fn read_entry(path: &Path, name: &str) -> Result<(Vec<u8>, &'static str)> {
    let mut zip = open_zip(path)?;
    read_entry_from(&mut zip, name)
}

pub fn read_entry_from<R: Read + Seek>(
    zip: &mut zip::ZipArchive<R>,
    name: &str,
) -> Result<(Vec<u8>, &'static str)> {
    let mut entry = zip.by_name(name)?;
    let mut buf = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut buf)?;
    Ok((buf, content_type_for(name)))
}

pub struct ZipIndex {
    path: PathBuf,
    by_name: HashMap<String, CdRecord>,
}

impl ZipIndex {
    pub fn open(path: &Path) -> Result<ZipIndex> {
        let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        let size = file.metadata()?.len();
        let by_name = central_directory(&mut file, size)?
            .into_iter()
            .map(|r| (r.name.clone(), r))
            .collect();
        Ok(ZipIndex {
            path: path.to_path_buf(),
            by_name,
        })
    }

    pub fn read_entry(&self, name: &str) -> Result<(Vec<u8>, &'static str)> {
        let rec = self
            .by_name
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("entry not found: {name}"))?;
        match self.read_positioned(rec) {
            Ok(bytes) => Ok((bytes, content_type_for(name))),
            Err(e) => {
                tracing::warn!(
                    "positioned read of {name} in {} failed ({e}); falling back to zip crate",
                    self.path.display()
                );
                read_entry(&self.path, name)
            }
        }
    }

    fn read_positioned(&self, rec: &CdRecord) -> Result<Vec<u8>> {
        use anyhow::bail;
        if rec.flags & 0x0001 != 0 {
            bail!("encrypted entry"); // bit 0 = encrypted, unsupported, fall back
        }
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(rec.local_header_offset))?;
        let mut lh = [0u8; 30];
        file.read_exact(&mut lh)?;
        if u32::from_le_bytes(lh[0..4].try_into().unwrap()) != 0x0403_4b50 {
            bail!("bad local file header signature");
        }
        let name_len = u16::from_le_bytes(lh[26..28].try_into().unwrap()) as u64;
        let extra_len = u16::from_le_bytes(lh[28..30].try_into().unwrap()) as u64;
        let data_start = rec.local_header_offset + 30 + name_len + extra_len;
        file.seek(SeekFrom::Start(data_start))?;
        let mut compressed = vec![0u8; rec.compressed_size as usize];
        file.read_exact(&mut compressed)?;

        let raw = match rec.compression {
            0 => compressed, // STORED (comic JPEGs are usually stored)
            8 => {
                let mut out = Vec::with_capacity(rec.uncompressed_size as usize);
                flate2::read::DeflateDecoder::new(&compressed[..]).read_to_end(&mut out)?;
                out
            }
            other => bail!("unsupported compression method {other}"),
        };

        if raw.len() as u64 != rec.uncompressed_size {
            bail!(
                "size mismatch: got {}, want {}",
                raw.len(),
                rec.uncompressed_size
            );
        }
        if crc32fast::hash(&raw) != rec.crc32 {
            bail!("crc mismatch");
        }
        Ok(raw)
    }
}

fn is_image(name: &str) -> bool {
    matches!(ext_lower(name).as_deref(), Some(ext) if IMAGE_EXTS.contains(&ext))
}

fn ext_lower(name: &str) -> Option<String> {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
}

pub(crate) fn content_type_for(name: &str) -> &'static str {
    match ext_lower(name).as_deref() {
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("avif") => "image/avif",
        Some("bmp") => "image/bmp",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    fn zip_bytes(entries: &[&str]) -> Vec<u8> {
        let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        for name in entries {
            z.start_file(*name, opts).unwrap();
            z.write_all(b"x").unwrap();
        }
        z.finish().unwrap().into_inner()
    }

    fn zip_bytes_with_payload(entries: &[&str], payload: usize) -> Vec<u8> {
        let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        let data = vec![b'x'; payload];
        for name in entries {
            z.start_file(*name, opts).unwrap();
            z.write_all(&data).unwrap();
        }
        z.finish().unwrap().into_inner()
    }

    fn zip_with(entries: &[(&str, &[u8])], method: zip::CompressionMethod) -> Vec<u8> {
        let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default().compression_method(method);
        for (name, data) in entries {
            z.start_file(*name, opts).unwrap();
            z.write_all(data).unwrap();
        }
        z.finish().unwrap().into_inner()
    }

    #[test]
    fn unpadded_numeric_pages_sort_naturally_not_lexically() {
        let names: Vec<String> = (1..=20).map(|n| format!("{n}.jpg")).collect();
        let mut shuffled: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        shuffled.sort_by_key(|s| s.len());
        let bytes = zip_bytes(&shuffled);

        let (inspection, _) = inspect_reader(Cursor::new(bytes)).unwrap();
        assert_eq!(
            inspection.pages, names,
            "pages must read in natural numeric order"
        );
    }

    #[test]
    fn zero_padded_pages_still_sort_correctly() {
        let names = vec!["001.jpg", "002.jpg", "010.jpg", "011.jpg"];
        let bytes = zip_bytes(&names);
        let (inspection, _) = inspect_reader(Cursor::new(bytes)).unwrap();
        assert_eq!(inspection.pages, names);
    }

    fn temp_zip(tag: &str, bytes: &[u8]) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "arca-archive-test-{}-{}.zip",
            tag,
            std::process::id()
        ));
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn hand_parsed_names_match_the_zip_crate() {
        let entries = [
            "10.jpg",
            "2.jpg",
            "1.jpg",
            "cover.png",
            "notes.txt",
            "extra/9.webp",
        ];
        let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        z.set_raw_comment(
            b"a comment containing a stray PK\x05\x06 sig"
                .to_vec()
                .into(),
        );
        for name in entries {
            z.start_file(name, opts).unwrap();
            z.write_all(b"payload-bytes").unwrap();
        }
        let bytes = z.finish().unwrap().into_inner();
        let path = temp_zip("match", &bytes);

        let got = list_pages(&path).unwrap();

        let mut want: Vec<String> = open_zip(&path)
            .unwrap()
            .file_names()
            .filter(|n| is_image(n))
            .map(str::to_string)
            .collect();
        want.sort_by(|a, b| crate::media::series::natural_cmp(a, b));

        std::fs::remove_file(&path).ok();
        assert_eq!(got, want, "hand-parsed page list must match the zip crate");
        assert_eq!(
            got,
            vec!["1.jpg", "2.jpg", "10.jpg", "cover.png", "extra/9.webp"]
        );
    }

    #[test]
    fn hand_parse_handles_many_entries() {
        let names: Vec<String> = (1..=300).map(|n| format!("p{n:04}.jpg")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let path = temp_zip("many", &zip_bytes(&refs));
        let got = list_pages(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(got, names);
    }

    struct Counting<R> {
        inner: R,
        reads: std::rc::Rc<std::cell::Cell<u64>>,
        seeks: std::rc::Rc<std::cell::Cell<u64>>,
    }
    impl<R> Counting<R> {
        fn new(inner: R) -> Self {
            Counting {
                inner,
                reads: Default::default(),
                seeks: Default::default(),
            }
        }
        fn counters(
            &self,
        ) -> (
            std::rc::Rc<std::cell::Cell<u64>>,
            std::rc::Rc<std::cell::Cell<u64>>,
        ) {
            (self.reads.clone(), self.seeks.clone())
        }
    }
    impl<R: Read> Read for Counting<R> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.reads.set(self.reads.get() + 1);
            self.inner.read(buf)
        }
    }
    impl<R: Seek> Seek for Counting<R> {
        fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
            self.seeks.set(self.seeks.get() + 1);
            self.inner.seek(pos)
        }
    }

    #[test]
    fn list_pages_io_is_bounded_not_per_entry() {
        let big = zip_bytes(
            &(0..1000)
                .map(|n| format!("{n:05}.jpg"))
                .collect::<Vec<_>>()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        );
        let size = big.len() as u64;

        let mut counting = Counting::new(Cursor::new(big));
        let (reads_c, seeks_c) = counting.counters();
        let names = central_directory_names(&mut counting, size).unwrap();
        assert_eq!(names.len(), 1000, "must still read every entry name");

        let seeks = seeks_c.get();
        let reads = reads_c.get();
        assert!(
            seeks <= 10,
            "central-directory parse must be O(1) in seeks, got {seeks} for 1000 entries \
             (a per-entry-seek regression?)"
        );
        assert!(reads <= 10, "and O(1) in reads, got {reads}");
    }

    #[test]
    fn opening_to_read_one_page_costs_a_seek_per_entry() {
        const N: usize = 1000;
        let names: Vec<String> = (0..N).map(|i| format!("{i:05}.jpg")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let bytes = zip_bytes_with_payload(&refs, 4096);

        let reader = Counting::new(Cursor::new(bytes));
        let (_reads, seeks) = reader.counters();
        let mut zip = zip::ZipArchive::new(reader).unwrap();
        let open_seeks = seeks.get();

        let before = seeks.get();
        let (page, _) = read_entry_from(&mut zip, &names[N / 2]).unwrap();
        let read_seeks = seeks.get() - before;
        assert_eq!(page.len(), 4096, "the page's uncompressed bytes");

        assert!(
            open_seeks as usize >= N,
            "the cold OPEN seeks to every entry's local header: {open_seeks} seeks for {N} \
             entries — this is what the first page read pays on a remote link"
        );
        assert!(
            read_seeks <= 5,
            "reading one page from the ALREADY-OPEN handle is cheap ({read_seeks} seeks), so \
             the cost is the open, not the read — the fix is to make the open cheap"
        );
    }

    #[test]
    fn zip_index_reads_match_the_zip_crate() {
        let e2: Vec<u8> = (0..3000u32)
            .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
            .collect();
        let entries: Vec<(&str, Vec<u8>)> = vec![
            ("001.jpg", vec![0xFF; 5000]),
            ("002.png", e2),
            ("sub/003.webp", b"tiny".to_vec()),
            ("meta.txt", b"not an image".to_vec()),
            ("004.jpg", Vec::new()),
        ];
        for method in [
            zip::CompressionMethod::Stored,
            zip::CompressionMethod::Deflated,
        ] {
            let refs: Vec<(&str, &[u8])> =
                entries.iter().map(|(n, d)| (*n, d.as_slice())).collect();
            let path = temp_zip(&format!("idx-{method:?}"), &zip_with(&refs, method));

            let index = ZipIndex::open(&path).unwrap();
            let mut zc = open_zip(&path).unwrap();
            for (name, data) in &entries {
                let (got, ct) = index.read_entry(name).unwrap();
                let (want, ct_crate) = read_entry_from(&mut zc, name).unwrap();
                assert_eq!(&got, &want, "{name} ({method:?}) must match the zip crate");
                assert_eq!(
                    &got, data,
                    "{name} ({method:?}) must equal the original bytes"
                );
                assert_eq!(ct, ct_crate);
            }
            assert!(
                index.read_entry("does-not-exist.jpg").is_err(),
                "an unknown entry errors, never panics"
            );
            std::fs::remove_file(&path).ok();
        }
    }

    fn local_content_archives() -> Vec<std::path::PathBuf> {
        fn is_archive(p: &std::path::Path) -> bool {
            matches!(
                p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_lowercase())
                    .as_deref(),
                Some("zip" | "cbz" | "cbr")
            )
        }
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(rd) = std::fs::read_dir(dir) else {
                return;
            };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if is_archive(&p) {
                    out.push(p);
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("content");
        let mut out = Vec::new();
        walk(&root, &mut out);
        out
    }

    #[test]
    fn zip_index_matches_crate_on_local_content() {
        let mut ran = 0;
        for path in local_content_archives() {
            let Ok(mut zc) = open_zip(&path) else {
                continue;
            };
            ran += 1;
            let names = list_pages(&path).unwrap();
            let index = ZipIndex::open(&path).unwrap();
            let step = (names.len() / 8).max(1);
            for name in names.iter().step_by(step) {
                let (got, _) = index.read_entry(name).unwrap();
                let (want, _) = read_entry_from(&mut zc, name).unwrap();
                assert_eq!(
                    got,
                    want,
                    "{name} in {} must match the zip crate",
                    path.display()
                );
            }
        }
        eprintln!("zip_index_matches_crate_on_local_content: checked {ran} real archive(s)");
    }

    #[test]
    fn corrupted_entry_never_serves_wrong_bytes() {
        let good = zip_with(
            &[("001.jpg", &vec![0xAB; 2000])],
            zip::CompressionMethod::Stored,
        );
        let mut bad = good.clone();
        bad[100] ^= 0xFF;
        let path = temp_zip("corrupt", &bad);
        let index = ZipIndex::open(&path).unwrap();
        let res = index.read_entry("001.jpg");
        std::fs::remove_file(&path).ok();
        assert!(
            res.is_err(),
            "a corrupted entry must error, never serve wrong bytes (got {} bytes)",
            res.map(|(b, _)| b.len()).unwrap_or(0)
        );
    }

    fn build_archive(
        files: &[(&str, &[u8])],
        dirs: &[&str],
        method: zip::CompressionMethod,
    ) -> Vec<u8> {
        let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default().compression_method(method);
        for d in dirs {
            z.add_directory(*d, opts).unwrap();
        }
        for (name, data) in files {
            z.start_file(*name, opts).unwrap();
            z.write_all(data).unwrap();
        }
        z.finish().unwrap().into_inner()
    }

    fn assert_inspect_matches(bytes: Vec<u8>, label: &str) {
        let (want, _) = inspect_reader_via_crate(Cursor::new(bytes.clone())).unwrap();
        let mut cur = Cursor::new(bytes);
        let size = cur.seek(SeekFrom::End(0)).unwrap();
        let got = inspection_from_records(&central_directory(&mut cur, size).unwrap());
        assert_eq!(got.pages, want.pages, "{label}: pages");
        assert_eq!(
            got.structural_hash, want.structural_hash,
            "{label}: structural_hash (the identity bucket key!)"
        );
        assert_eq!(got.is_epub, want.is_epub, "{label}: is_epub");
        assert_eq!(got.epub_hash, want.epub_hash, "{label}: epub_hash");
    }

    #[test]
    fn inspect_via_cd_matches_crate() {
        for method in [
            zip::CompressionMethod::Stored,
            zip::CompressionMethod::Deflated,
        ] {
            assert_inspect_matches(
                build_archive(
                    &[
                        ("10.jpg", b"j10"),
                        ("2.jpg", b"j2"),
                        ("1.jpg", b"j1"),
                        ("cover.png", b"png"),
                        ("read me.txt", b"notes"),
                    ],
                    &["pages/"],
                    method,
                ),
                &format!("comic/{method:?}"),
            );
            assert_inspect_matches(
                build_archive(
                    &[
                        ("META-INF/container.xml", b"<container/>"),
                        ("OEBPS/ch1.xhtml", b"<html/>"),
                        ("OEBPS/cover.jpg", b"img-bytes"),
                    ],
                    &["META-INF/", "OEBPS/"],
                    method,
                ),
                &format!("epub-with-image/{method:?}"),
            );
            assert_inspect_matches(
                build_archive(
                    &[
                        ("META-INF/container.xml", b"<container/>"),
                        ("OEBPS/ch1.xhtml", b"<html>a</html>"),
                    ],
                    &["META-INF/", "OEBPS/"],
                    method,
                ),
                &format!("epub-text-only/{method:?}"),
            );
        }
    }

    #[test]
    fn inspect_matches_crate_on_local_content() {
        let mut ran = 0;
        for path in local_content_archives() {
            let rel = path.display();
            let Ok((want, _)) = inspect_reader_via_crate(File::open(&path).unwrap()) else {
                continue;
            };
            ran += 1;
            let (got, _) = inspect_reader(File::open(&path).unwrap()).unwrap();
            assert_eq!(
                got.structural_hash, want.structural_hash,
                "{rel}: structural_hash"
            );
            assert_eq!(got.epub_hash, want.epub_hash, "{rel}: epub_hash");
            assert_eq!(got.is_epub, want.is_epub, "{rel}: is_epub");
            assert_eq!(got.pages, want.pages, "{rel}: pages");
        }
        eprintln!("inspect_matches_crate_on_local_content: checked {ran} real archive(s)");
    }

    #[test]
    fn cd_is_file_matches_documented_classification() {
        let rec = |name: &str, version_made_by: u16, external_attributes: u32| CdRecord {
            name: name.into(),
            flags: 0,
            compression: 0,
            crc32: 0,
            compressed_size: 0,
            uncompressed_size: 0,
            local_header_offset: 0,
            version_made_by,
            external_attributes,
        };
        assert!(!cd_is_file(&rec("pages/", 0, 0)));
        assert!(!cd_is_file(&rec("pages\\", 0, 0)));
        assert!(!cd_is_file(&rec("link", 3 << 8, 0o120777 << 16)));
        assert!(cd_is_file(&rec("a.jpg", 3 << 8, 0o100644 << 16)));
        assert!(cd_is_file(&rec("a.jpg", 0, 0)));
        assert!(cd_is_file(&rec("a.jpg", 0, 0x20)));
    }
}
