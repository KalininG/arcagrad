// Export-only code appears unused in host-target tests.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

const CATALOG_URL: &str = "https://www.marxists.org/ebooks/index.htm";
const ORIGIN: &str = "https://www.marxists.org";
const PAGE_SIZE: usize = 25;
const USER_AGENT: &str = "arcagrad-marxists/0.1 (+https://github.com/arcagrad/arcagrad)";

use arcagrad_plugin_sdk::{
    BrowseItem, BrowsePage, Feed, MappedTag, PluginManifest, RateLimit, RateRule, RawTag,
    ReferenceInput, ScrapedMetadata, CONTRACT_VERSION, MANIFEST_VERSION,
};

fn manifest_doc() -> PluginManifest {
    PluginManifest {
        manifest_version: MANIFEST_VERSION,
        id: "marxists".into(),
        version: "0.1.0".into(),
        author: "KalininG".into(),
        icon: None,
        repository: Some(
            "https://github.com/KalininG/arcagrad/tree/main/plugins/marxists".into(),
        ),
        name: "Marxists Internet Archive".into(),
        description:
            "Browse and download the curated English EPUB collection from Marxists Internet Archive."
                .into(),
        source: "marxists".into(),
        capabilities: vec!["browse".into(), "download".into()],
        hosts: vec!["marxists.org".into()],
        auth: None,
        rate_limit: Some(RateLimit {
            rules: vec![RateRule {
                match_pattern: String::new(),
                requests: 1,
                per_ms: 1000,
            }],
            max_concurrency: 1,
        }),
        feeds: vec![Feed {
            id: "catalog".into(),
            label: "Catalog".into(),
            ranges: Vec::new(),
            query: true,
            auth: false,
            cache_ttl: 21600,
        }],
        reference_inputs: BTreeMap::from([(
            "download".into(),
            ReferenceInput {
                label: "Marxists.org EPUB URLs".into(),
                placeholder: "One direct marxists.org .epub URL per line".into(),
                help: "Paste one or more direct EPUB links from Marxists Internet Archive.".into(),
                required: true,
            },
        )]),
        item_cache_ttl: 86400,
        image_headers: BTreeMap::new(),
        clean_titles: false,
        followable: false,
        reading_mode: "paged".into(),
        nsfw: false,
        contract_version: CONTRACT_VERSION,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct BookRef {
    url: String,
    title: String,
    author: String,
}

fn class_has(node: roxmltree::Node<'_, '_>, class: &str) -> bool {
    node.attribute("class")
        .is_some_and(|v| v.split_whitespace().any(|c| c == class))
}

fn node_text(node: roxmltree::Node<'_, '_>) -> String {
    node.descendants()
        .filter(|n| n.is_text())
        .filter_map(|n| n.text())
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_path(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    format!("/{}", parts.join("/"))
}

fn resolve_href(href: &str) -> Option<String> {
    let href = href.trim();
    let path = if let Some(rest) = href.strip_prefix("https://www.marxists.org") {
        rest
    } else if let Some(rest) = href.strip_prefix("http://www.marxists.org") {
        rest
    } else if href.starts_with("http://") || href.starts_with("https://") {
        return None;
    } else if href.starts_with('/') {
        href
    } else {
        return Some(format!(
            "{ORIGIN}{}",
            normalize_path(&format!("/ebooks/{href}"))
        ));
    };
    Some(format!("{ORIGIN}{}", normalize_path(path)))
}

fn is_epub_url(url: &str) -> bool {
    url.split(['?', '#'])
        .next()
        .is_some_and(|p| p.to_ascii_lowercase().ends_with(".epub"))
}

fn parse_row(row_xml: &str, books: &mut Vec<BookRef>) -> Result<(), String> {
    let wrapped = format!("<root>{}</root>", row_xml.replace("&nbsp;", "&#160;"));
    let doc = roxmltree::Document::parse(&wrapped).map_err(|e| format!("bad MIA row: {e}"))?;
    let row = doc.root_element();
    let Some(head) = row
        .descendants()
        .find(|n| n.has_tag_name("p") && class_has(*n, "head"))
    else {
        return Ok(());
    };
    let author = node_text(head);
    let Some(note) = row
        .descendants()
        .find(|n| n.has_tag_name("p") && class_has(*n, "note"))
    else {
        return Ok(());
    };
    let mut title_parts: Vec<String> = Vec::new();
    for child in note.children() {
        if child.is_text() {
            if let Some(t) = child.text() {
                title_parts.push(t.to_string());
            }
            continue;
        }
        if !child.is_element() {
            continue;
        }
        match child.tag_name().name() {
            "br" => title_parts.clear(),
            "a" => {
                let Some(href) = child.attribute("href") else {
                    continue;
                };
                let Some(url) = resolve_href(href) else {
                    continue;
                };
                if !is_epub_url(&url) {
                    continue;
                }
                let title = title_parts
                    .join(" ")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                if !title.is_empty() && !author.is_empty() {
                    books.push(BookRef {
                        url,
                        title,
                        author: author.clone(),
                    });
                }
                title_parts.clear();
            }
            _ => title_parts.push(node_text(child)),
        }
    }
    Ok(())
}

fn parse_catalog(html: &str) -> Result<Vec<BookRef>, String> {
    let lower = html.to_ascii_lowercase();
    let mut books = Vec::new();
    let mut cursor = 0;
    while let Some(relative_start) = lower[cursor..].find("<tr") {
        let start = cursor + relative_start;
        let Some(relative_end) = lower[start..].find("</tr>") else {
            break;
        };
        let end = start + relative_end + "</tr>".len();
        parse_row(&html[start..end], &mut books)?;
        cursor = end;
    }
    Ok(books)
}

fn encode_reference(book: &BookRef) -> String {
    serde_json::to_string(book).expect("BookRef serialization cannot fail")
}

fn decode_reference(reference: &str) -> Option<BookRef> {
    if let Ok(book) = serde_json::from_str(reference) {
        return Some(book);
    }
    let url = resolve_href(reference)?;
    if !is_epub_url(&url) {
        return None;
    }
    let stem = url
        .split(['?', '#'])
        .next()?
        .rsplit('/')
        .next()?
        .strip_suffix(".epub")?
        .replace(['_', '-'], " ");
    Some(BookRef {
        url,
        title: stem.split_whitespace().collect::<Vec<_>>().join(" "),
        author: String::new(),
    })
}

fn metadata(book: &BookRef) -> ScrapedMetadata {
    let mut raw_tags = Vec::new();
    let mut mapped_tags = vec![MappedTag {
        namespace: "language".into(),
        value: "english".into(),
        qualifier: "none".into(),
        role: "none".into(),
    }];
    if !book.author.is_empty() {
        raw_tags.push(RawTag {
            namespace: "author".into(),
            value: book.author.clone(),
        });
        mapped_tags.push(MappedTag {
            namespace: "creator".into(),
            value: book.author.clone(),
            qualifier: "none".into(),
            role: "none".into(),
        });
    }
    ScrapedMetadata {
        title: (!book.title.is_empty()).then(|| book.title.clone()),
        language: Some("english".into()),
        description: None,
        // The direct EPUB is the only unique per-book source URL in this catalog.
        source_url: Some(book.url.clone()),
        raw_tags,
        mapped_tags,
        ..Default::default()
    }
}

fn filename(title: &str) -> String {
    let safe: String = title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_') {
                c
            } else {
                ' '
            }
        })
        .collect();
    let safe = safe.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("{}.epub", if safe.is_empty() { "book" } else { &safe })
}

fn browse_page(all: Vec<BookRef>, query: Option<&str>, page: u32) -> BrowsePage {
    let query = query.unwrap_or_default().trim().to_lowercase();
    let filtered: Vec<BookRef> = all
        .into_iter()
        .filter(|b| {
            query.is_empty()
                || b.title.to_lowercase().contains(&query)
                || b.author.to_lowercase().contains(&query)
        })
        .collect();
    let pages = filtered.len().div_ceil(PAGE_SIZE).max(1);
    let page = page.max(1) as usize;
    let start = (page - 1).saturating_mul(PAGE_SIZE);
    let items = filtered
        .into_iter()
        .skip(start)
        .take(PAGE_SIZE)
        .map(|book| BrowseItem {
            reference: encode_reference(&book),
            title: book.title.clone(),
            cover_url: String::new(),
            page_count: None,
            favorites: None,
            rating: None,
            subtitle: Some(book.author.clone()),
            source_url: Some(book.url.clone()),
        })
        .collect();
    BrowsePage {
        items,
        num_pages: Some(pages as i64),
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::*;
    use arcagrad_plugin_sdk::{guest, BrowseRequest, Candidate, DownloadPlan, HttpFetchRequest};
    use extism_pdk::*;

    fn get(url: &str) -> Result<String, Error> {
        let mut req = HttpFetchRequest::get(url);
        req.headers.insert("User-Agent".into(), USER_AGENT.into());
        let response = guest::fetch(&req)?;
        if response.status != 200 {
            return Err(Error::msg(format!(
                "MIA GET {url} -> HTTP {}",
                response.status
            )));
        }
        Ok(response.body)
    }

    #[plugin_fn]
    pub fn manifest(_input: String) -> FnResult<String> {
        Ok(serde_json::to_string(&manifest_doc())?)
    }

    #[plugin_fn]
    pub fn icon(_input: String) -> FnResult<Vec<u8>> {
        Ok(include_bytes!("../icon.webp").to_vec())
    }

    #[plugin_fn]
    pub fn browse(input: String) -> FnResult<String> {
        let request: BrowseRequest = serde_json::from_str(&input)?;
        if request.feed != "catalog" {
            return Err(Error::msg("unknown Marxists.org feed").into());
        }
        let html = get(CATALOG_URL)?;
        let books = parse_catalog(&html).map_err(Error::msg)?;
        Ok(serde_json::to_string(&browse_page(
            books,
            request.query.as_deref(),
            request.page,
        ))?)
    }

    #[plugin_fn]
    pub fn fetch_details(input: String) -> FnResult<String> {
        let candidate: Candidate = serde_json::from_str(&input)?;
        let book = decode_reference(&candidate.id)
            .ok_or_else(|| Error::msg("not a Marxists.org EPUB reference"))?;
        Ok(serde_json::to_string(&metadata(&book))?)
    }

    #[plugin_fn]
    pub fn download(input: String) -> FnResult<String> {
        let candidate: Candidate = serde_json::from_str(&input)?;
        let book = decode_reference(&candidate.id)
            .ok_or_else(|| Error::msg("not a Marxists.org EPUB reference"))?;
        Ok(serde_json::to_string(&DownloadPlan {
            url: book.url.clone(),
            filename: filename(&book.title),
            metadata: metadata(&book),
            ..Default::default()
        })?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_is_strict_valid() {
        let errors = arcagrad_plugin_sdk::validate_manifest(&manifest_doc());
        assert!(errors.is_empty(), "manifest invalid: {errors:?}");
    }

    const CATALOG: &str = r#"<?xml version="1.0"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body><table>
<tr><td><p class="head"><a href="../archive/lenin/index.htm">V. I. Lenin</a></p>
<p class="note">The State and Revolution <a href="lenin/state-and-revolution.epub">epub</a>
<a href="lenin/state-and-revolution.mobi">mobi</a><br />
What is to be Done? <a href="lenin/what-is-to-be-done.epub">epub</a><br /></p></td></tr>
<tr><td><p class="head"><a>Chris Harman</a></p><p class="note">
Debates in State Capitalism <em>(with Mandel &amp; Kidron)</em>
<a href="harman/debates.epub">epub</a><a href="harman/debates.pdf">pdf</a><br /></p></td></tr>
</table><p>&nbsp;</p></body></html>"#;

    #[test]
    fn parses_titles_authors_and_direct_epub_urls() {
        let books = parse_catalog(CATALOG).unwrap();
        assert_eq!(books.len(), 3);
        assert_eq!(books[0].title, "The State and Revolution");
        assert_eq!(books[0].author, "V. I. Lenin");
        assert_eq!(
            books[0].url,
            "https://www.marxists.org/ebooks/lenin/state-and-revolution.epub"
        );
        assert_eq!(
            books[2].title,
            "Debates in State Capitalism (with Mandel & Kidron)"
        );
    }

    #[test]
    fn filters_and_paginates_locally() {
        let books = parse_catalog(CATALOG).unwrap();
        let page = browse_page(books, Some("lenin"), 1);
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.num_pages, Some(1));
        assert!(page
            .items
            .iter()
            .all(|i| i.subtitle.as_deref() == Some("V. I. Lenin")));
    }

    #[test]
    fn opaque_reference_round_trips_into_download_metadata() {
        let book = BookRef {
            url: "https://www.marxists.org/ebooks/lenin/state-and-revolution.epub".into(),
            title: "The State and Revolution".into(),
            author: "V. I. Lenin".into(),
        };
        let decoded = decode_reference(&encode_reference(&book)).unwrap();
        assert_eq!(decoded, book);
        let meta = metadata(&decoded);
        assert!(meta
            .mapped_tags
            .iter()
            .any(|t| t.namespace == "creator" && t.value == "V. I. Lenin"));
        assert_eq!(filename(&decoded.title), "The State and Revolution.epub");
        assert_eq!(meta.source_url.as_deref(), Some(book.url.as_str()));
    }

    #[test]
    fn rejects_non_mia_and_non_epub_manual_urls() {
        assert!(decode_reference("https://example.com/book.epub").is_none());
        assert!(decode_reference("https://www.marxists.org/book.pdf").is_none());
        assert!(decode_reference(
            "https://www.marxists.org/ebooks/lenin/state-and-revolution.epub"
        )
        .is_some());
    }

    #[test]
    #[ignore]
    fn parses_current_live_catalog_fixture() {
        let path = std::env::var("MIA_CATALOG_FIXTURE").expect("set MIA_CATALOG_FIXTURE");
        let html = std::fs::read_to_string(path).unwrap();
        let books = parse_catalog(&html).unwrap();
        assert!(
            books.len() > 100,
            "unexpectedly small catalog: {}",
            books.len()
        );
        assert!(books.iter().any(|b| b.title == "The State and Revolution"));
    }
}
