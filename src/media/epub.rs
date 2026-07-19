//! EPUB metadata, spine, cover, and table-of-contents parsing.

use std::io::Read;
use std::path::Path;

use anyhow::{anyhow, Context, Result};

/// EPUB metadata and reading structure. Hrefs are resolved ZIP entry paths.
#[derive(Debug, Clone, PartialEq)]
pub struct EpubMeta {
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub language: Option<String>,
    /// Decoded `dc:description` value.
    pub description: Option<String>,
    /// ZIP path of the cover image, if the OPF declares one.
    pub cover_href: Option<String>,
    /// Content-document ZIP paths in reading order (the spine).
    pub spine: Vec<String>,
    pub reflowable: bool,
    /// Compact ISBN-10 or ISBN-13 from `dc:identifier`.
    pub isbn: Option<String>,
    pub series_index: Option<f64>,
    pub publisher: Option<String>,
    /// Flattened EPUB3 navigation or EPUB2 NCX table of contents.
    pub toc: Vec<TocEntry>,
}

/// Flattened table-of-contents entry with a resolved href.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocEntry {
    pub label: String,
    pub href: String,
    pub level: usize,
}

pub fn split_creators(creator: &str) -> Vec<(String, &'static str)> {
    creator
        .split(['/', ';', '&'])
        .flat_map(|p| p.split(" and "))
        .flat_map(split_credit)
        .map(|(s, role)| (deinvert_name(&s), role))
        .collect()
}

const CO_CREATOR_CREDITS: &[&str] = &[
    "illustrated by",
    "illustrations by",
    "illustration by",
    "art by",
    "artwork by",
    "drawn by",
    "cover art by",
    "cover by",
];
const CONTRIBUTOR_CREDITS: &[&str] = &[
    "translated by",
    "translation by",
    "edited by",
    "foreword by",
    "introduction by",
    "afterword by",
    "preface by",
    "notes by",
];

fn find_credit(haystack: &str, needle: &str) -> Option<usize> {
    let (h, n) = (haystack.as_bytes(), needle.as_bytes());
    if n.is_empty() || h.len() < n.len() {
        return None;
    }
    (0..=h.len() - n.len()).find(|&i| {
        let before = i == 0 || !h[i - 1].is_ascii_alphanumeric();
        let after = i + n.len() == h.len() || !h[i + n.len()].is_ascii_alphanumeric();
        before
            && after
            && h[i..i + n.len()]
                .iter()
                .zip(n)
                .all(|(a, b)| a.to_ascii_lowercase() == *b)
    })
}

fn split_credit(creator: &str) -> Vec<(String, &'static str)> {
    let end = CONTRIBUTOR_CREDITS
        .iter()
        .filter_map(|c| find_credit(creator, c))
        .min()
        .unwrap_or(creator.len());
    let mut names = Vec::new();
    let mut rest = &creator[..end];
    let mut lead = true;
    loop {
        let role = if lead { "author" } else { "illustrator" };
        match CO_CREATOR_CREDITS
            .iter()
            .filter_map(|c| find_credit(rest, c).map(|i| (i, c.len())))
            .min_by_key(|&(i, _)| i)
        {
            Some((i, len)) => {
                names.push((clean_credit(&rest[..i]), role));
                lead = false;
                rest = &rest[i + len..];
            }
            None => {
                names.push((clean_credit(rest), role));
                break;
            }
        }
    }
    names.into_iter().filter(|(s, _)| !s.is_empty()).collect()
}

fn clean_credit(s: &str) -> String {
    s.trim().trim_matches(',').trim().to_string()
}

fn deinvert_name(name: &str) -> String {
    let name = name.trim();
    let (last, first) = match name.split_once(',') {
        Some((l, r)) => (l.trim(), r.trim()),
        None => return name.to_string(),
    };
    if last.is_empty() || first.is_empty() {
        return name.to_string();
    }
    const NOT_A_GIVEN_NAME: &[&str] = &[
        "by",
        "illustrated",
        "edited",
        "translated",
        "trans",
        "editor",
        "translator",
        "foreword",
        "introduction",
        "afterword",
        "jr",
        "sr",
        "ii",
        "iii",
        "iv",
        "phd",
        "md",
        "ed",
        "eds",
        "esq",
    ];
    let first_words: Vec<String> = first
        .split_whitespace()
        .map(|w| w.trim_matches('.').to_ascii_lowercase())
        .collect();
    if first_words
        .iter()
        .any(|w| NOT_A_GIVEN_NAME.contains(&w.as_str()))
    {
        return name.to_string();
    }
    if last.split_whitespace().count() > 3 || first_words.len() > 4 {
        return name.to_string();
    }
    format!("{first} {last}")
}

pub fn normalize_language(lang: &str) -> String {
    let l = lang.trim().to_ascii_lowercase();
    let base = l.split(['-', '_']).next().unwrap_or(&l);
    match base {
        "en" | "eng" | "english" => "english",
        "ja" | "jpn" | "japanese" => "japanese",
        "zh" | "chi" | "zho" | "chinese" => "chinese",
        "ko" | "kor" | "korean" => "korean",
        "fr" | "fra" | "fre" | "french" => "french",
        "de" | "deu" | "ger" | "german" => "german",
        "es" | "spa" | "spanish" => "spanish",
        "it" | "ita" | "italian" => "italian",
        "ru" | "rus" | "russian" => "russian",
        "pt" | "por" | "portuguese" => "portuguese",
        other => other,
    }
    .to_string()
}

pub fn parse_isbn(raw: &str) -> Option<String> {
    let compact: String = raw
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == 'X' || *c == 'x')
        .collect::<String>()
        .to_ascii_uppercase();
    match compact.len() {
        13 if compact.bytes().all(|b| b.is_ascii_digit()) => Some(compact),
        10 if compact[..9].bytes().all(|b| b.is_ascii_digit()) => Some(compact),
        _ => None,
    }
}

struct ManifestItem {
    href: String,
    media_type: String,
    properties: String,
}

fn parse_xml(s: &str) -> std::result::Result<roxmltree::Document<'_>, roxmltree::Error> {
    roxmltree::Document::parse_with_options(
        s,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
}

pub fn count_words(path: &Path, spine: &[String]) -> Result<u64> {
    const MAX_SLURP: u64 = 128 * 1024 * 1024;
    if std::fs::metadata(path)?.len() > MAX_SLURP {
        return Ok(0);
    }
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).context("open epub zip")?;
    let mut chars: u64 = 0;
    let mut buf = Vec::new();
    for entry in spine {
        buf.clear();
        if let Ok(mut e) = zip.by_name(entry) {
            if e.read_to_end(&mut buf).is_ok() {
                chars += text_chars(&String::from_utf8_lossy(&buf));
            }
        }
    }
    Ok(chars / 5)
}

fn text_chars(html: &str) -> u64 {
    let b = html.as_bytes();
    let n = b.len();
    let ci = |i: usize, needle: &[u8]| -> bool {
        b.len() >= i + needle.len() && b[i..i + needle.len()].eq_ignore_ascii_case(needle)
    };
    let mut count: u64 = 0;
    let mut i = 0;
    while i < n {
        if b[i] == b'<' {
            let close: Option<&[u8]> = if ci(i, b"<script") {
                Some(b"</script")
            } else if ci(i, b"<style") {
                Some(b"</style")
            } else {
                None
            };
            i += 1;
            if let Some(close) = close {
                while i < n && !ci(i, close) {
                    i += 1;
                }
            }
            while i < n && b[i] != b'>' {
                i += 1;
            }
            if i < n {
                i += 1; // consume the '>'
            }
        } else {
            let ch = html[i..].chars().next().unwrap();
            if !ch.is_whitespace() {
                count += 1;
            }
            i += ch.len_utf8();
        }
    }
    count
}

fn read_entry(index: &crate::media::archive::ZipIndex, name: &str) -> Result<String> {
    let (bytes, _) = index
        .read_entry(name)
        .with_context(|| format!("epub entry {name}"))?;
    String::from_utf8(bytes).with_context(|| format!("epub entry {name} not utf-8"))
}

fn clean_description(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let lower = raw.to_ascii_lowercase();
    let mut paragraphs: Vec<&str> = Vec::new();
    let mut cursor = 0;
    while let Some(relative_start) = lower[cursor..].find("<p") {
        let start = cursor + relative_start;
        let Some(open_relative_end) = lower[start..].find('>') else {
            break;
        };
        let content_start = start + open_relative_end + 1;
        let Some(close_relative) = lower[content_start..].find("</p>") else {
            break;
        };
        let end = content_start + close_relative + "</p>".len();
        paragraphs.push(&raw[start..end]);
        cursor = end;
    }

    let Some(first_prose) = paragraphs.iter().position(|p| text_chars(p) >= 160) else {
        return Some(raw.to_string());
    };
    if first_prose == 0 {
        return Some(raw.to_string());
    }

    let cleaned = paragraphs[first_prose..]
        .iter()
        .filter(|p| {
            !p.to_ascii_lowercase()
                .contains("epub format revised and verified")
        })
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    (!cleaned.trim().is_empty()).then_some(cleaned)
}

/// Parse metadata and reading structure from a valid EPUB.
pub fn inspect(path: &Path) -> Result<EpubMeta> {
    let index = crate::media::archive::ZipIndex::open(path).context("open epub zip")?;

    let container = read_entry(&index, "META-INF/container.xml")?;
    let opf_path = {
        let doc = parse_xml(&container).context("parse container.xml")?;
        doc.descendants()
            .find(|n| n.tag_name().name() == "rootfile") // ignore XML namespace
            .and_then(|n| n.attribute("full-path"))
            .ok_or_else(|| anyhow!("epub container.xml has no rootfile"))?
            .to_string()
    };

    let opf = read_entry(&index, &opf_path)?;
    let doc = parse_xml(&opf).context("parse opf")?;

    let mut title = None;
    let mut authors: Vec<String> = Vec::new();
    let mut language = None;
    let mut description = None;
    let mut cover_id: Option<String> = None; // EPUB2 <meta name="cover" content="ID">
    let mut manifest: std::collections::HashMap<String, ManifestItem> =
        std::collections::HashMap::new();
    let mut spine_ids: Vec<String> = Vec::new();
    let mut isbn: Option<String> = None;
    let mut series_index: Option<f64> = None;
    let mut publisher: Option<String> = None;

    let text = |n: &roxmltree::Node| {
        n.text()
            .map(|t| t.trim().to_string())
            .filter(|s| !s.is_empty())
    };

    for n in doc.descendants() {
        match n.tag_name().name() {
            "title" if title.is_none() => title = text(&n), // dc:title
            "creator" => {
                if let Some(a) = text(&n) {
                    authors.push(a);
                }
            }
            "identifier" if isbn.is_none() => {
                isbn = n.text().and_then(parse_isbn);
            }
            "language" if language.is_none() => language = text(&n),
            "publisher" if publisher.is_none() => publisher = text(&n), // dc:publisher
            "description" if description.is_none() => {
                description = text(&n).and_then(|d| clean_description(&d))
            }
            "meta" if n.attribute("name") == Some("cover") => {
                cover_id = n.attribute("content").map(str::to_string);
            }
            "meta" if n.attribute("name") == Some("calibre:series_index") => {
                series_index = n
                    .attribute("content")
                    .and_then(|c| c.trim().parse::<f64>().ok());
            }
            "meta" if n.attribute("property") == Some("group-position") => {
                if series_index.is_none() {
                    series_index = n.text().and_then(|t| t.trim().parse::<f64>().ok());
                }
            }
            "item" => {
                if let (Some(id), Some(href)) = (n.attribute("id"), n.attribute("href")) {
                    manifest.insert(
                        id.to_string(),
                        ManifestItem {
                            href: href.to_string(),
                            media_type: n.attribute("media-type").unwrap_or("").to_string(),
                            properties: n.attribute("properties").unwrap_or("").to_string(),
                        },
                    );
                }
            }
            "itemref" => {
                if let Some(idref) = n.attribute("idref") {
                    spine_ids.push(idref.to_string());
                }
            }
            _ => {}
        }
    }

    let opf_dir = opf_path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let resolve = |href: &str| resolve_join(opf_dir, href);

    let cover_href = manifest
        .values()
        .find(|m| m.properties.split_whitespace().any(|p| p == "cover-image"))
        .map(|m| resolve(&m.href))
        .or_else(|| {
            cover_id
                .as_deref()
                .and_then(|id| manifest.get(id))
                .map(|m| resolve(&m.href))
        });

    let spine: Vec<String> = spine_ids
        .iter()
        .filter_map(|id| manifest.get(id))
        .map(|m| resolve(&m.href))
        .collect();
    if spine.is_empty() {
        return Err(anyhow!("epub has an empty/unresolvable spine"));
    }

    let reflowable = spine_ids
        .iter()
        .filter_map(|id| manifest.get(id))
        .any(|m| m.media_type.contains("xhtml") || m.media_type.contains("html"));

    let toc = parse_toc(&index, &doc, &manifest, opf_dir).unwrap_or_default();

    Ok(EpubMeta {
        title,
        authors,
        language,
        description,
        cover_href,
        spine,
        reflowable,
        isbn,
        series_index,
        publisher,
        toc,
    })
}

fn resolve_join(base_dir: &str, href: &str) -> String {
    let (path, frag) = match href.split_once('#') {
        Some((p, f)) => (p, Some(f)),
        None => (href, None),
    };
    let mut segs: Vec<&str> = if base_dir.is_empty() {
        Vec::new()
    } else {
        base_dir.split('/').collect()
    };
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                segs.pop();
            }
            s => segs.push(s),
        }
    }
    let joined = segs.join("/");
    match frag {
        Some(f) => format!("{joined}#{f}"),
        None => joined,
    }
}

fn parse_toc(
    index: &crate::media::archive::ZipIndex,
    opf: &roxmltree::Document,
    manifest: &std::collections::HashMap<String, ManifestItem>,
    opf_dir: &str,
) -> Option<Vec<TocEntry>> {
    let dir_of = |zip_path: &str| -> String {
        zip_path
            .rsplit_once('/')
            .map(|(d, _)| d.to_string())
            .unwrap_or_default()
    };
    if let Some(nav) = manifest
        .values()
        .find(|m| m.properties.split_whitespace().any(|p| p == "nav"))
    {
        let nav_zip = resolve_join(opf_dir, &nav.href);
        if let Ok(xml) = read_entry(index, &nav_zip) {
            if let Ok(d) = parse_xml(&xml) {
                let entries = parse_nav(&d, &dir_of(&nav_zip));
                if !entries.is_empty() {
                    return Some(entries);
                }
            }
        }
    }
    let ncx_id = opf
        .descendants()
        .find(|n| n.tag_name().name() == "spine")
        .and_then(|n| n.attribute("toc"))?;
    let ncx = manifest.get(ncx_id)?;
    let ncx_zip = resolve_join(opf_dir, &ncx.href);
    let xml = read_entry(index, &ncx_zip).ok()?;
    let d = parse_xml(&xml).ok()?;
    let entries = parse_ncx(&d, &dir_of(&ncx_zip));
    (!entries.is_empty()).then_some(entries)
}

/// EPUB3 nav: walk the toc `<nav>`'s nested `<ol>/<li>/<a>`; level = `<ol>` nesting depth.
fn parse_nav(doc: &roxmltree::Document, base_dir: &str) -> Vec<TocEntry> {
    let nav = doc
        .descendants()
        .find(|n| {
            n.tag_name().name() == "nav"
                && n.attributes()
                    .any(|a| a.name() == "type" && a.value() == "toc")
        })
        .or_else(|| doc.descendants().find(|n| n.tag_name().name() == "nav"));
    let Some(nav) = nav else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for a in nav.descendants().filter(|n| n.tag_name().name() == "a") {
        let Some(href) = a.attribute("href") else {
            continue;
        };
        let label = a
            .descendants()
            .filter(|n| n.is_text())
            .filter_map(|n| n.text())
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if label.is_empty() {
            continue;
        }
        let level = a
            .ancestors()
            .filter(|n| n.tag_name().name() == "ol")
            .count()
            .saturating_sub(1);
        out.push(TocEntry {
            label,
            href: resolve_join(base_dir, href),
            level,
        });
    }
    out
}

/// EPUB2 NCX: walk `<navMap>`'s nested `<navPoint>`; level = `<navPoint>` nesting depth.
fn parse_ncx(doc: &roxmltree::Document, base_dir: &str) -> Vec<TocEntry> {
    let Some(nav_map) = doc.descendants().find(|n| n.tag_name().name() == "navMap") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for np in nav_map
        .descendants()
        .filter(|n| n.tag_name().name() == "navPoint")
    {
        let src = np
            .children()
            .find(|n| n.tag_name().name() == "content")
            .and_then(|n| n.attribute("src"));
        let label = np
            .children()
            .find(|n| n.tag_name().name() == "navLabel")
            .and_then(|nl| nl.children().find(|n| n.tag_name().name() == "text"))
            .and_then(|t| t.text())
            .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_default();
        let (Some(src), false) = (src, label.is_empty()) else {
            continue;
        };
        let level = np
            .ancestors()
            .filter(|n| n.tag_name().name() == "navPoint")
            .count()
            .saturating_sub(1);
        out.push(TocEntry {
            label,
            href: resolve_join(base_dir, src),
            level,
        });
    }
    out
}

pub fn cover_entry(path: &Path) -> Result<String> {
    if let Some(href) = inspect(path)?.cover_href {
        return Ok(href);
    }
    crate::media::archive::list_pages(path)?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("epub declares no cover and contains no images"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    pub(crate) fn write_epub(path: &Path, entries: &[(&str, &[u8])]) {
        let f = std::fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            z.start_file(*name, opts).unwrap();
            z.write_all(data).unwrap();
        }
        z.finish().unwrap();
    }

    fn epub3_fixture(path: &Path) {
        let container = br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>The Great Book</dc:title>
    <dc:creator>Jane Author</dc:creator>
    <dc:creator>Second Author</dc:creator>
    <dc:language>en</dc:language>
    <description xmlns="http://purl.org/dc/elements/1.1/">&lt;p&gt;A useful synopsis.&lt;/p&gt;</description>
  </metadata>
  <manifest>
    <item id="cover-img" href="images/cover.jpg" media-type="image/jpeg" properties="cover-image"/>
    <item id="ch1" href="text/ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="ch2" href="text/ch2.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="ch1"/>
    <itemref idref="ch2"/>
  </spine>
</package>"#;
        write_epub(
            path,
            &[
                ("mimetype", b"application/epub+zip"),
                ("META-INF/container.xml", container),
                ("OEBPS/content.opf", opf),
                ("OEBPS/images/cover.jpg", b"\xff\xd8\xffdummy-jpeg"),
                (
                    "OEBPS/text/ch1.xhtml",
                    b"<html><body>Chapter 1</body></html>",
                ),
                (
                    "OEBPS/text/ch2.xhtml",
                    b"<html><body>Chapter 2</body></html>",
                ),
            ],
        );
    }

    #[test]
    fn inspects_an_epub3_metadata_spine_and_cover() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("book.epub");
        epub3_fixture(&p);

        let m = inspect(&p).unwrap();
        assert_eq!(m.title.as_deref(), Some("The Great Book"));
        assert_eq!(m.authors, vec!["Jane Author", "Second Author"]);
        assert_eq!(m.language.as_deref(), Some("en"));
        assert_eq!(m.description.as_deref(), Some("<p>A useful synopsis.</p>"));
        assert_eq!(m.cover_href.as_deref(), Some("OEBPS/images/cover.jpg"));
        assert_eq!(
            m.spine,
            vec![
                "OEBPS/text/ch1.xhtml".to_string(),
                "OEBPS/text/ch2.xhtml".to_string()
            ]
        );
        assert!(m.reflowable, "xhtml spine → reflowable");
        assert!(m.toc.is_empty(), "this fixture ships no nav/ncx");
    }

    #[test]
    fn spine_and_cover_normalize_dotdot_hrefs() {
        let container = br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/opf/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Updir Book</dc:title>
  </metadata>
  <manifest>
    <item id="cover-img" href="../images/cover.jpg" media-type="image/jpeg" properties="cover-image"/>
    <item id="ch1" href="../text/ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="ch2" href="./ch2.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="ch1"/>
    <itemref idref="ch2"/>
  </spine>
</package>"#;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("updir.epub");
        write_epub(
            &p,
            &[
                ("mimetype", b"application/epub+zip"),
                ("META-INF/container.xml", container),
                ("OEBPS/opf/content.opf", opf),
                ("OEBPS/opf/ch2.xhtml", b"<html><body>2</body></html>"),
                ("OEBPS/images/cover.jpg", b"\xff\xd8\xffdummy-jpeg"),
                ("OEBPS/text/ch1.xhtml", b"<html><body>1</body></html>"),
            ],
        );

        let m = inspect(&p).unwrap();
        assert_eq!(m.cover_href.as_deref(), Some("OEBPS/images/cover.jpg"));
        assert_eq!(
            m.spine,
            vec![
                "OEBPS/text/ch1.xhtml".to_string(),
                "OEBPS/opf/ch2.xhtml".to_string()
            ]
        );
    }

    #[test]
    fn description_skips_calibre_metadata_rows_before_synopsis() {
        let synopsis = "Over a century and a half after its publication, Moby-Dick still stands as an indisputable literary classic. It is the story of an eerily compelling madman pursuing an unholy war against a creature as vast and dangerous and unknowable as the sea itself.";
        let raw = format!(
            "<p>Genre: Challenge, Fiction, Literature</p>\
             <p>ebook, 861 pages</p><div><p>Paperback, 654 pages</p>\
             <p>Published: 1851</p><p>Goodreads Best Books of the 19th Century</p>\
             <p>Illustrations by: Rockwell Kent</p><p>{synopsis}</p>\
             <p>This edition includes a foreword and explanatory commentary for readers.</p></div>\
             <p>Jun 2023 - epub format revised and verified by zardox.</p>"
        );

        let cleaned = clean_description(&raw).unwrap();
        assert!(cleaned.starts_with("<p>Over a century and a half"));
        assert!(!cleaned.contains("Genre:"));
        assert!(!cleaned.contains("Paperback"));
        assert!(!cleaned.contains("revised and verified"));
        assert!(cleaned.contains("This edition includes"));
    }

    #[test]
    fn description_leaves_normal_html_and_plain_text_unchanged() {
        let html = "<p>This is an intentionally long publisher synopsis that begins immediately and should remain byte-for-byte unchanged because there are no metadata rows before it. It contains enough prose to cross the conservative detection threshold safely.</p>";
        assert_eq!(clean_description(html).as_deref(), Some(html));

        let plain = "A short plain-text publisher synopsis.";
        assert_eq!(clean_description(plain).as_deref(), Some(plain));
    }

    #[test]
    fn parses_epub3_nav_toc_with_nesting() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("nav.epub");
        let container = br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Nav Book</dc:title><dc:language>en</dc:language></metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="c1" href="text/ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="c2" href="text/ch2.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine><itemref idref="c1"/><itemref idref="c2"/></spine>
</package>"#;
        let nav = br#"<?xml version="1.0"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><body>
  <nav epub:type="toc"><ol>
    <li><a href="text/ch1.xhtml">Chapter One</a>
      <ol><li><a href="text/ch1.xhtml#s2"> Section  Two </a></li></ol></li>
    <li><a href="text/ch2.xhtml">Chapter Two</a></li>
  </ol></nav></body></html>"#;
        write_epub(
            &p,
            &[
                ("mimetype", b"application/epub+zip"),
                ("META-INF/container.xml", container),
                ("OEBPS/content.opf", opf),
                ("OEBPS/nav.xhtml", nav),
                ("OEBPS/text/ch1.xhtml", b"<html><body>1</body></html>"),
                ("OEBPS/text/ch2.xhtml", b"<html><body>2</body></html>"),
            ],
        );
        let m = inspect(&p).unwrap();
        assert_eq!(
            m.toc,
            vec![
                TocEntry {
                    label: "Chapter One".into(),
                    href: "OEBPS/text/ch1.xhtml".into(),
                    level: 0
                },
                TocEntry {
                    label: "Section Two".into(),
                    href: "OEBPS/text/ch1.xhtml#s2".into(),
                    level: 1
                },
                TocEntry {
                    label: "Chapter Two".into(),
                    href: "OEBPS/text/ch2.xhtml".into(),
                    level: 0
                },
            ]
        );
        assert!(m.spine.contains(&"OEBPS/text/ch1.xhtml".to_string()));
    }

    #[test]
    fn parses_epub2_ncx_toc() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("ncx.epub");
        let container = br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Legacy</dc:title><dc:language>en</dc:language></metadata>
  <manifest>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
    <item id="c1" href="text/ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="c2" href="text/ch2.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine toc="ncx"><itemref idref="c1"/><itemref idref="c2"/></spine>
</package>"#;
        let ncx = br#"<?xml version="1.0"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1"><navMap>
  <navPoint id="n1"><navLabel><text>Chapter One</text></navLabel><content src="text/ch1.xhtml"/>
    <navPoint id="n1a"><navLabel><text>Part A</text></navLabel><content src="text/ch1.xhtml#a"/></navPoint></navPoint>
  <navPoint id="n2"><navLabel><text>Chapter Two</text></navLabel><content src="text/ch2.xhtml"/></navPoint>
</navMap></ncx>"#;
        write_epub(
            &p,
            &[
                ("mimetype", b"application/epub+zip"),
                ("META-INF/container.xml", container),
                ("OEBPS/content.opf", opf),
                ("OEBPS/toc.ncx", ncx),
                ("OEBPS/text/ch1.xhtml", b"<html><body>1</body></html>"),
                ("OEBPS/text/ch2.xhtml", b"<html><body>2</body></html>"),
            ],
        );
        let m = inspect(&p).unwrap();
        assert_eq!(
            m.toc,
            vec![
                TocEntry {
                    label: "Chapter One".into(),
                    href: "OEBPS/text/ch1.xhtml".into(),
                    level: 0
                },
                TocEntry {
                    label: "Part A".into(),
                    href: "OEBPS/text/ch1.xhtml#a".into(),
                    level: 1
                },
                TocEntry {
                    label: "Chapter Two".into(),
                    href: "OEBPS/text/ch2.xhtml".into(),
                    level: 0
                },
            ]
        );
    }

    #[test]
    fn text_chars_strips_markup_and_counts_nonwhitespace() {
        assert_eq!(text_chars("<p>Hello world</p>"), 10);
        assert_eq!(text_chars("<b>a</b> <i>b</i>"), 2);
        assert_eq!(text_chars("<style>p { color: red }</style>Hi"), 2);
        assert_eq!(
            text_chars("<script>alert('boom'.repeat(9))</script>done"),
            4
        );
        assert_eq!(text_chars("Café résumé"), 10);
    }

    #[test]
    fn count_words_is_spine_chars_over_five() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("wc.epub");
        let container = br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>WC</dc:title><dc:language>en</dc:language></metadata>
  <manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/><item id="c2" href="c2.xhtml" media-type="application/xhtml+xml"/></manifest>
  <spine><itemref idref="c1"/><itemref idref="c2"/></spine>
</package>"#;
        let doc = format!("<html><body><p>{}</p></body></html>", "x".repeat(50));
        write_epub(
            &p,
            &[
                ("mimetype", b"application/epub+zip"),
                ("META-INF/container.xml", container),
                ("OEBPS/content.opf", opf),
                ("OEBPS/c1.xhtml", doc.as_bytes()),
                ("OEBPS/c2.xhtml", doc.as_bytes()),
            ],
        );
        let m = inspect(&p).unwrap();
        assert_eq!(count_words(&p, &m.spine).unwrap(), 20);
    }

    #[test]
    fn epub2_cover_via_meta_pointer() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("legacy.epub");
        let container = br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;
        let opf = br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Old Book</dc:title>
    <meta name="cover" content="the-cover"/>
  </metadata>
  <manifest>
    <item id="the-cover" href="cover.png" media-type="image/png"/>
    <item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine><itemref idref="c1"/></spine>
</package>"#;
        write_epub(
            &p,
            &[
                ("META-INF/container.xml", container),
                ("content.opf", opf),
                ("cover.png", b"\x89PNGdummy"),
                ("c1.xhtml", b"<html/>"),
            ],
        );
        let m = inspect(&p).unwrap();
        assert_eq!(m.title.as_deref(), Some("Old Book"));
        assert_eq!(
            m.cover_href.as_deref(),
            Some("cover.png"),
            "root OPF → href as-is"
        );
        assert_eq!(m.spine, vec!["c1.xhtml".to_string()]);
    }

    #[test]
    fn cover_entry_prefers_declared_cover_then_falls_back() {
        let dir = tempfile::tempdir().unwrap();

        let p1 = dir.path().join("declared.epub");
        epub3_fixture(&p1);
        assert_eq!(cover_entry(&p1).unwrap(), "OEBPS/images/cover.jpg");

        let p2 = dir.path().join("undeclared.epub");
        write_epub(
            &p2,
            &[
                (
                    "META-INF/container.xml",
                    br#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
                ),
                (
                    "content.opf",
                    br#"<package xmlns="http://www.idpf.org/2007/opf"><manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#,
                ),
                ("art.png", b"\x89PNGdummy"),
                ("c1.xhtml", b"<html/>"),
            ],
        );
        assert_eq!(cover_entry(&p2).unwrap(), "art.png");

        let p3 = dir.path().join("textonly.epub");
        write_epub(
            &p3,
            &[
                (
                    "META-INF/container.xml",
                    br#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
                ),
                (
                    "content.opf",
                    br#"<package xmlns="http://www.idpf.org/2007/opf"><manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#,
                ),
                ("c1.xhtml", b"<html/>"),
            ],
        );
        assert!(cover_entry(&p3).is_err());
    }

    #[test]
    fn a_plain_zip_is_not_an_epub() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("comic.cbz");
        write_epub(&p, &[("001.jpg", b"not-an-epub")]);
        assert!(
            inspect(&p).is_err(),
            "no container.xml → not a readable epub"
        );
    }

    fn c(name: &str, role: &'static str) -> (String, &'static str) {
        (name.to_string(), role)
    }

    #[test]
    fn splits_combined_creators() {
        assert_eq!(
            split_creators("Asato Asato and Shirabii"),
            vec![c("Asato Asato", "author"), c("Shirabii", "author")]
        );
        assert_eq!(
            split_creators("A / B ; C"),
            vec![c("A", "author"), c("B", "author"), c("C", "author")]
        );
        assert_eq!(
            split_creators("Cormac McCarthy"),
            vec![c("Cormac McCarthy", "author")]
        );
        assert_eq!(
            split_creators("Alexander Dumas"),
            vec![c("Alexander Dumas", "author")]
        );
    }

    #[test]
    fn normalizes_language_codes() {
        assert_eq!(normalize_language("en"), "english");
        assert_eq!(normalize_language("en-US"), "english");
        assert_eq!(normalize_language("JA"), "japanese");
        assert_eq!(normalize_language("Klingon"), "klingon");
    }

    #[test]
    fn parses_only_real_isbns() {
        assert_eq!(
            parse_isbn("9781975303136").as_deref(),
            Some("9781975303136")
        );
        assert_eq!(
            parse_isbn("urn:isbn:978-1-9753-0313-6").as_deref(),
            Some("9781975303136")
        );
        assert_eq!(parse_isbn("080442957X").as_deref(), Some("080442957X"));
        assert_eq!(parse_isbn("urn:uuid:1234-5678"), None);
        assert_eq!(parse_isbn("42"), None);
    }

    #[test]
    fn inspect_extracts_isbn_from_identifier() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("book.epub");
        write_epub(
            &p,
            &[
                (
                    "META-INF/container.xml",
                    br#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
                ),
                (
                    "content.opf",
                    br#"<package xmlns="http://www.idpf.org/2007/opf"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>T</dc:title><dc:identifier>urn:uuid:abc</dc:identifier><dc:identifier>9781975303136</dc:identifier><dc:language>en</dc:language></metadata><manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#,
                ),
                ("c1.xhtml", b"<html/>"),
            ],
        );
        let m = inspect(&p).unwrap();
        assert_eq!(
            m.isbn.as_deref(),
            Some("9781975303136"),
            "picks the ISBN, skips the UUID"
        );
    }

    fn inspect_with_metadata(meta_xml: &str) -> EpubMeta {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("book.epub");
        let opf = format!(
            r#"<package xmlns="http://www.idpf.org/2007/opf"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>T</dc:title>{meta_xml}</metadata><manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#
        );
        write_epub(
            &p,
            &[
                (
                    "META-INF/container.xml",
                    br#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
                ),
                ("content.opf", opf.as_bytes()),
                ("c1.xhtml", b"<html/>"),
            ],
        );
        inspect(&p).unwrap()
    }

    #[test]
    fn deinverts_last_first_names_but_leaves_credits_and_naturals() {
        assert_eq!(deinvert_name("Woolf, Virginia"), "Virginia Woolf");
        assert_eq!(deinvert_name("McCarthy, Cormac"), "Cormac McCarthy");
        assert_eq!(deinvert_name("Lenin, V. I."), "V. I. Lenin");
        assert_eq!(deinvert_name("Cormac McCarthy"), "Cormac McCarthy");
        assert_eq!(deinvert_name("David Bradshaw"), "David Bradshaw");
        assert_eq!(
            deinvert_name("Nisioisin, Illustrated by Vofan"),
            "Nisioisin, Illustrated by Vofan"
        );
        assert_eq!(deinvert_name("King, Jr."), "King, Jr.");
    }

    #[test]
    fn split_creators_deinverts_each_person() {
        assert_eq!(
            split_creators("Woolf, Virginia"),
            vec![c("Virginia Woolf", "author")]
        );
        assert_eq!(
            split_creators("Asato Asato and Shirabii"),
            vec![c("Asato Asato", "author"), c("Shirabii", "author")]
        );
        assert_eq!(
            split_creators("Nisioisin, Illustrated by Vofan"),
            vec![c("Nisioisin", "author"), c("Vofan", "illustrator")]
        );
    }

    #[test]
    fn split_credit_handles_packed_credits_conservatively() {
        assert_eq!(
            split_creators("Nisioisin, Illustrated by Vofan"),
            vec![c("Nisioisin", "author"), c("Vofan", "illustrator")]
        );
        assert_eq!(
            split_creators("Asato Asato, Art by Shirabii"),
            vec![c("Asato Asato", "author"), c("Shirabii", "illustrator")]
        );
        assert_eq!(
            split_creators("Woolf, Virginia, Edited by David Bradshaw"),
            vec![c("Virginia Woolf", "author")]
        );
        assert_eq!(
            split_creators("Homer, Translated by Robert Fagles"),
            vec![c("Homer", "author")]
        );
        assert_eq!(
            split_creators("Cormac McCarthy"),
            vec![c("Cormac McCarthy", "author")]
        );
        assert_eq!(
            split_creators("Bart Byrne"),
            vec![c("Bart Byrne", "author")]
        );
    }

    #[test]
    fn inspect_reads_publisher() {
        let m = inspect_with_metadata(r#"<dc:publisher>Kodansha comics</dc:publisher>"#);
        assert_eq!(m.publisher.as_deref(), Some("Kodansha comics"));
        assert_eq!(inspect_with_metadata("").publisher, None);
    }

    #[test]
    fn inspect_reads_calibre_series_index() {
        let m = inspect_with_metadata(
            r#"<meta name="calibre:series" content="Monogatari Series"/><meta name="calibre:series_index" content="4.0"/>"#,
        );
        assert_eq!(m.series_index, Some(4.0));

        assert_eq!(inspect_with_metadata("").series_index, None);
    }

    #[test]
    fn inspect_reads_epub3_group_position_and_calibre_wins() {
        let m = inspect_with_metadata(
            r##"<meta property="belongs-to-collection" id="c1">Monogatari</meta><meta refines="#c1" property="group-position">2</meta>"##,
        );
        assert_eq!(m.series_index, Some(2.0));

        let both = inspect_with_metadata(
            r#"<meta property="group-position">2</meta><meta name="calibre:series_index" content="7"/>"#,
        );
        assert_eq!(both.series_index, Some(7.0));
    }
}
