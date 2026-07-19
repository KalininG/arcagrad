//! Lenient ComicInfo.xml extraction for scan-time metadata ingest.

use std::io::Read;
use std::path::Path;

use anyhow::{anyhow, Result};

/// Sanity cap on the XML entry — a legitimate ComicInfo is a few KB.
const MAX_XML_BYTES: u64 = 1024 * 1024;

/// Parsed fields consumed by metadata ingest or retained for series detection.
#[derive(Debug, Default, PartialEq)]
pub struct ComicInfo {
    pub series: Option<String>,
    pub title: Option<String>,
    pub number: Option<String>,
    pub volume: Option<String>,
    pub count: Option<String>,
    pub summary: Option<String>,
    /// Canonical source URL (`Web`) — lands in `item_sources` as provenance.
    pub web: Option<String>,
    pub language_iso: Option<String>,
    /// Merged `Genre` and `Tags` values.
    pub content_tags: Vec<String>,
    pub creators: Vec<(String, String)>,
}

/// The creator elements we ingest and the open-set `role` each maps to.
const CREATOR_FIELDS: &[(&str, &str)] = &[
    ("Writer", "writer"),
    ("Penciller", "penciller"),
    ("Inker", "inker"),
    ("Colorist", "colorist"),
    ("Letterer", "letterer"),
    ("CoverArtist", "cover artist"),
];

pub fn parse(xml: &str) -> Option<ComicInfo> {
    let xml = xml.trim_start_matches('\u{feff}');
    let doc = roxmltree::Document::parse(xml).ok()?;
    let root = doc.root_element();
    if !root.tag_name().name().eq_ignore_ascii_case("ComicInfo") {
        return None;
    }

    let mut info = ComicInfo::default();
    for node in root.children().filter(|n| n.is_element()) {
        let name = node.tag_name().name();
        let text = node.text().map(str::trim).filter(|t| !t.is_empty());
        let owned = |t: &str| Some(t.to_string());
        if name.eq_ignore_ascii_case("Series") {
            info.series = text.and_then(owned);
        } else if name.eq_ignore_ascii_case("Title") {
            info.title = text.and_then(owned);
        } else if name.eq_ignore_ascii_case("Number") {
            info.number = text.and_then(owned);
        } else if name.eq_ignore_ascii_case("Volume") {
            info.volume = text.and_then(owned);
        } else if name.eq_ignore_ascii_case("Count") {
            info.count = text.and_then(owned);
        } else if name.eq_ignore_ascii_case("Summary") {
            info.summary = text.and_then(owned);
        } else if name.eq_ignore_ascii_case("Web") {
            info.web = text.and_then(owned);
        } else if name.eq_ignore_ascii_case("LanguageISO") {
            info.language_iso = text.and_then(owned);
        } else if name.eq_ignore_ascii_case("Genre") || name.eq_ignore_ascii_case("Tags") {
            if let Some(t) = text {
                info.content_tags.extend(split_list(t));
            }
        } else {
            for (field, role) in CREATOR_FIELDS {
                if name.eq_ignore_ascii_case(field) {
                    if let Some(t) = text {
                        info.creators
                            .extend(split_list(t).into_iter().map(|n| (n, role.to_string())));
                    }
                }
            }
        }
    }
    Some(info)
}

/// Split comma- or semicolon-separated ComicInfo values.
fn split_list(value: &str) -> Vec<String> {
    value
        .split([',', ';'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn read_from_archive(path: &Path) -> Result<Option<String>> {
    let mut magic = [0u8; 4];
    {
        let mut f = std::fs::File::open(path)?;
        let n = f.read(&mut magic)?;
        if n < 4 {
            return Ok(None);
        }
    }
    if &magic == b"Rar!" {
        read_from_rar(path)
    } else {
        read_from_zip(path)
    }
}

/// True when this entry name is a ComicInfo.xml (any directory, any case).
fn is_comicinfo_name(name: &str) -> bool {
    name.rsplit(['/', '\\'])
        .next()
        .is_some_and(|base| base.eq_ignore_ascii_case("ComicInfo.xml"))
}

/// Depth key for "shallowest wins" (then lexicographic for determinism).
fn depth_of(name: &str) -> usize {
    name.matches(['/', '\\']).count()
}

fn read_from_zip(path: &Path) -> Result<Option<String>> {
    let file = std::fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| anyhow!("open zip: {e}"))?;
    let best: Option<String> = zip
        .file_names()
        .filter(|n| is_comicinfo_name(n))
        .min_by_key(|n| (depth_of(n), n.to_string()))
        .map(str::to_string);
    let Some(name) = best else { return Ok(None) };
    let mut entry = zip
        .by_name(&name)
        .map_err(|e| anyhow!("read zip entry {name:?}: {e}"))?;
    if entry.size() > MAX_XML_BYTES {
        return Ok(None);
    }
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut bytes)?;
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

fn read_from_rar(path: &Path) -> Result<Option<String>> {
    let mut names: Vec<String> = Vec::new();
    let archive = unrar::Archive::new(path)
        .open_for_listing()
        .map_err(|e| anyhow!("open rar for listing: {e}"))?;
    for header in archive {
        let header = header.map_err(|e| anyhow!("list rar: {e}"))?;
        if header.is_file() {
            let name = header.filename.to_string_lossy().into_owned();
            if is_comicinfo_name(&name) {
                names.push(name);
            }
        }
    }
    let Some(name) = names.into_iter().min_by_key(|n| (depth_of(n), n.clone())) else {
        return Ok(None);
    };
    let (bytes, _) = crate::media::rar::read_page(path, &name)?;
    if bytes.len() as u64 > MAX_XML_BYTES {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parses_the_standard_template() {
        let xml = r#"<?xml version="1.0"?>
<ComicInfo xmlns:xsd="http://www.w3.org/2001/XMLSchema">
  <Title>Issue Title</Title>
  <Series>Some Series</Series>
  <Number>3</Number>
  <Volume>2</Volume>
  <Count>12</Count>
  <Summary>A story &amp; a summary.</Summary>
  <Writer>Frank Miller, Klaus Janson</Writer>
  <Penciller>Frank Miller</Penciller>
  <Inker>Klaus Janson</Inker>
  <Colorist>Lynn Varley</Colorist>
  <Letterer>John Costanza</Letterer>
  <CoverArtist>Frank Miller</CoverArtist>
  <Editor>Dick Giordano</Editor>
  <Translator>Nobody</Translator>
  <Publisher>DC</Publisher>
  <Genre>Crime, Superhero</Genre>
  <Tags>gritty; noir</Tags>
  <LanguageISO>en</LanguageISO>
  <PageCount>223</PageCount>
  <ScanInformation></ScanInformation>
</ComicInfo>"#;
        let info = parse(xml).unwrap();
        assert_eq!(info.series.as_deref(), Some("Some Series"));
        assert_eq!(info.number.as_deref(), Some("3"));
        assert_eq!(info.summary.as_deref(), Some("A story & a summary."));
        assert_eq!(info.language_iso.as_deref(), Some("en"));
        assert_eq!(info.content_tags, ["Crime", "Superhero", "gritty", "noir"]);
        assert!(info
            .creators
            .contains(&("Frank Miller".into(), "writer".into())));
        assert!(info
            .creators
            .contains(&("Klaus Janson".into(), "writer".into())));
        assert!(info
            .creators
            .contains(&("Lynn Varley".into(), "colorist".into())));
        assert!(info
            .creators
            .contains(&("Frank Miller".into(), "cover artist".into())));
        assert!(!info.creators.iter().any(|(n, _)| n == "Dick Giordano"));
        assert!(!info.creators.iter().any(|(n, _)| n == "Nobody"));
    }

    #[test]
    fn tolerates_bom_garbage_and_wrong_roots() {
        assert!(parse("\u{feff}<ComicInfo><Series>S</Series></ComicInfo>").is_some());
        assert!(parse("not xml at all").is_none());
        assert!(parse("<SomethingElse><Series>S</Series></SomethingElse>").is_none());
        let info = parse("<ComicInfo><Summary></Summary><Web>  </Web></ComicInfo>").unwrap();
        assert!(info.summary.is_none() && info.web.is_none());
    }

    #[test]
    fn real_library_archives_extract_cleanly() {
        let root = Path::new("content");
        if !root.is_dir() {
            return;
        }
        for entry in walkdir::WalkDir::new(root).into_iter().flatten() {
            let p = entry.path();
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !matches!(ext.to_ascii_lowercase().as_str(), "cbz" | "zip" | "cbr") {
                continue;
            }
            match read_from_archive(p) {
                Ok(Some(xml)) => {
                    assert!(parse(&xml).is_some(), "unparseable ComicInfo in {p:?}")
                }
                Ok(None) => {}
                Err(e) => eprintln!("skipping unreadable {p:?}: {e:#}"),
            }
        }
    }

    fn write_zip(path: &Path, entries: &[(&str, &str)]) {
        let f = std::fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, body) in entries {
            z.start_file(*name, opts).unwrap();
            z.write_all(body.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }

    #[test]
    fn extraction_prefers_the_shallowest_entry() {
        let dir = tempfile::tempdir().unwrap();
        let cbz = dir.path().join("a.cbz");
        write_zip(
            &cbz,
            &[
                ("pages/001.jpg", "img"),
                (
                    "pages/comicinfo.XML",
                    "<ComicInfo><Series>nested</Series></ComicInfo>",
                ),
                (
                    "ComicInfo.xml",
                    "<ComicInfo><Series>root</Series></ComicInfo>",
                ),
            ],
        );
        let xml = read_from_archive(&cbz).unwrap().unwrap();
        assert_eq!(parse(&xml).unwrap().series.as_deref(), Some("root"));

        let bare = dir.path().join("b.cbz");
        write_zip(&bare, &[("001.jpg", "img")]);
        assert!(read_from_archive(&bare).unwrap().is_none());
    }
}
