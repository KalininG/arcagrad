use std::collections::{BTreeMap, BTreeSet};

const SEARCH: &str = "https://www.gutenberg.org/ebooks/search.opds/";
const USER_AGENT: &str = "arcagrad-gutenberg/0.2 (+https://github.com/arcagrad/arcagrad)";
const PAGE_SIZE: u32 = 25;

use arcagrad_plugin_sdk::{
    BrowseItem, BrowsePage, BrowseRequest, Candidate, DownloadPlan, Feed, MappedTag,
    PluginManifest, RateLimit, RateRule, RawTag, ReferenceInput, ScrapedMetadata, CONTRACT_VERSION,
    MANIFEST_VERSION,
};

fn manifest_doc() -> PluginManifest {
    PluginManifest {
        manifest_version: MANIFEST_VERSION,
        id: "gutenberg".into(),
        version: "0.1.0".into(),
        author: "KalininG".into(),
        icon: None,
        repository: Some(
            "https://github.com/KalininG/arcagrad/tree/main/plugins/gutenberg".into(),
        ),
        name: "Project Gutenberg".into(),
        description: "Browse and download 75,000+ free public-domain books (EPUB) from Project Gutenberg's own catalog."
            .into(),
        source: "gutenberg".into(),
        capabilities: vec!["browse".into(), "download".into()],
        hosts: vec!["gutenberg.org".into()],
        auth: None,
        rate_limit: Some(RateLimit {
            rules: vec![RateRule {
                match_pattern: String::new(),
                requests: 60,
                per_ms: 60_000,
            }],
            max_concurrency: 2,
        }),
        feeds: vec![
            Feed {
                id: "popular".into(),
                label: "Popular".into(),
                ranges: Vec::new(),
                query: true,
                auth: false,
                cache_ttl: 3600,
            },
            Feed {
                id: "recent".into(),
                label: "Recent".into(),
                ranges: Vec::new(),
                query: true,
                auth: false,
                cache_ttl: 3600,
            },
        ],
        reference_inputs: BTreeMap::from([(
            "download".into(),
            ReferenceInput {
                label: "Project Gutenberg book URLs or IDs".into(),
                placeholder: "One per line (e.g. 1342 or https://www.gutenberg.org/ebooks/1342)"
                    .into(),
                help: "Enter one or more Gutenberg book IDs (or ebook URLs) to download into the selected library type."
                    .into(),
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

fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn browse_url(feed: &str, query: Option<&str>, page: u32) -> String {
    let sort = if feed == "recent" {
        "release_date"
    } else {
        "downloads"
    };
    let mut url = format!("{SEARCH}?sort_order={sort}");
    if let Some(q) = query.map(str::trim).filter(|q| !q.is_empty()) {
        url.push_str(&format!("&query={}", enc(q)));
    }
    let page = page.max(1);
    if page > 1 {
        url.push_str(&format!("&start_index={}", (page - 1) * PAGE_SIZE + 1));
    }
    url
}

fn parse_book_id(reference: &str) -> Option<String> {
    let r = reference.trim();
    if !r.is_empty() && r.bytes().all(|b| b.is_ascii_digit()) {
        return Some(r.to_string());
    }
    r.split(['/', '?', '#', '.'])
        .rev()
        .find(|seg| !seg.is_empty() && seg.bytes().all(|b| b.is_ascii_digit()))
        .map(|s| s.to_string())
}

fn format_author(name: &str) -> String {
    match name.split_once(", ") {
        Some((last, first)) => format!("{} {}", first.trim(), last.trim()),
        None => name.trim().to_string(),
    }
}

fn source_url(id: &str) -> String {
    format!("https://www.gutenberg.org/ebooks/{id}")
}

fn cover_url(id: &str) -> String {
    format!("https://www.gutenberg.org/cache/epub/{id}/pg{id}.cover.medium.jpg")
}

fn language_name(code: &str) -> String {
    match code {
        "en" => "english",
        "fr" => "french",
        "de" => "german",
        "es" => "spanish",
        "it" => "italian",
        "pt" => "portuguese",
        "nl" => "dutch",
        "ru" => "russian",
        "zh" => "chinese",
        "ja" => "japanese",
        "la" => "latin",
        "el" => "greek",
        "fi" => "finnish",
        "sv" => "swedish",
        "hu" => "hungarian",
        "pl" => "polish",
        "da" => "danish",
        "no" => "norwegian",
        other => other,
    }
    .to_string()
}

fn subject_tags(subject: &str, out: &mut BTreeSet<String>) {
    for part in subject.split(" -- ") {
        let v = part.trim().to_lowercase();
        if v.is_empty() || matches!(v.as_str(), "fiction" | "fiction, general" | "general") {
            continue;
        }
        out.insert(v);
    }
}

fn epub_filename(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_') {
                c
            } else {
                ' '
            }
        })
        .collect();
    let base = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let base = if base.is_empty() {
        "book".to_string()
    } else {
        base
    };
    format!("{base}.epub")
}

fn parse_search(xml: &str) -> Result<Vec<BrowseItem>, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("bad OPDS feed: {e}"))?;
    let mut items = Vec::new();
    for entry in doc
        .descendants()
        .filter(|n| n.has_tag_name(("http://www.w3.org/2005/Atom", "entry")))
    {
        let text_of = |tag: &str| {
            entry
                .children()
                .find(|n| n.has_tag_name(("http://www.w3.org/2005/Atom", tag)))
                .and_then(|n| n.text())
                .map(str::trim)
                .unwrap_or("")
                .to_string()
        };
        let id_url = text_of("id");
        let Some(id) = id_url
            .strip_suffix(".opds")
            .and_then(|u| u.rsplit('/').next())
            .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        else {
            continue;
        };
        let title = text_of("title");
        if title.is_empty() {
            continue;
        }
        let author = text_of("content");
        items.push(BrowseItem {
            reference: id.to_string(),
            title,
            cover_url: cover_url(id),
            page_count: None,
            favorites: None,
            rating: None,
            subtitle: (!author.is_empty()).then_some(author),
            source_url: Some(source_url(id)),
        });
    }
    // Some feeds contain more entries than the 25-item paging stride.
    items.truncate(PAGE_SIZE as usize);
    Ok(items)
}

#[derive(Default, Debug)]
struct BookRecord {
    title: String,
    authors: Vec<String>,
    subjects: Vec<String>,
    language: Option<String>,
    downloads: Option<i64>,
    summary: Option<String>,
    epub: Option<String>,
    cover: Option<String>,
}

fn parse_book(xml: &str) -> Result<BookRecord, String> {
    const ATOM: &str = "http://www.w3.org/2005/Atom";
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("bad OPDS entry: {e}"))?;
    let mut rec = BookRecord::default();
    let mut authors = BTreeSet::new();
    let mut subjects = BTreeSet::new();
    let mut epubs: Vec<String> = Vec::new();

    for entry in doc
        .descendants()
        .filter(|n| n.has_tag_name((ATOM, "entry")))
    {
        if rec.title.is_empty() {
            if let Some(t) = entry
                .children()
                .find(|n| n.has_tag_name((ATOM, "title")))
                .and_then(|n| n.text())
            {
                rec.title = t.trim().to_string();
            }
        }
        for a in entry
            .children()
            .filter(|n| n.has_tag_name((ATOM, "author")))
        {
            if let Some(name) = a
                .children()
                .find(|n| n.has_tag_name((ATOM, "name")))
                .and_then(|n| n.text())
            {
                let name = name.trim();
                if !name.is_empty() {
                    authors.insert(format_author(name));
                }
            }
        }
        for c in entry
            .children()
            .filter(|n| n.has_tag_name((ATOM, "category")))
        {
            if c.attribute("scheme") == Some("http://purl.org/dc/terms/LCSH") {
                if let Some(term) = c.attribute("term") {
                    subjects.insert(term.to_string());
                }
            }
        }
        if rec.language.is_none() {
            if let Some(lang) = entry
                .children()
                .find(|n| n.has_tag_name(("http://purl.org/dc/terms/", "language")))
                .and_then(|n| n.text())
            {
                let lang = lang.trim();
                if !lang.is_empty() {
                    rec.language = Some(lang.to_string());
                }
            }
        }
        for l in entry.children().filter(|n| n.has_tag_name((ATOM, "link"))) {
            match (l.attribute("rel"), l.attribute("type"), l.attribute("href")) {
                (
                    Some("http://opds-spec.org/acquisition"),
                    Some("application/epub+zip"),
                    Some(h),
                ) => {
                    epubs.push(h.to_string());
                }
                (Some("http://opds-spec.org/image"), _, Some(h)) if rec.cover.is_none() => {
                    rec.cover = Some(h.to_string());
                }
                _ => {}
            }
        }
        // Download count and summary are embedded in the XHTML content field.
        if let Some(content) = entry.children().find(|n| n.has_tag_name((ATOM, "content"))) {
            for p in content
                .descendants()
                .filter(|n| n.is_element() && n.tag_name().name() == "p")
            {
                let text: String = p
                    .descendants()
                    .filter(|n| n.is_text())
                    .filter_map(|n| n.text())
                    .collect();
                let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
                if rec.downloads.is_none() {
                    if let Some(rest) = text.strip_prefix("Downloads:") {
                        rec.downloads = rest.trim().replace(',', "").parse::<i64>().ok();
                    }
                }
                if rec.summary.is_none() {
                    if let Some(rest) = text.strip_prefix("Summary:") {
                        let s = rest.trim();
                        if !s.is_empty() {
                            rec.summary = Some(s.to_string());
                        }
                    }
                }
            }
        }
    }

    rec.authors = authors.into_iter().collect();
    rec.subjects = subjects.into_iter().collect();
    rec.epub = epubs
        .iter()
        .find(|u| u.ends_with(".epub3.images"))
        .or_else(|| epubs.iter().find(|u| u.ends_with(".epub.images")))
        .or_else(|| epubs.first())
        .cloned();
    if rec.title.is_empty() {
        return Err("OPDS entry has no title".into());
    }
    Ok(rec)
}

fn book_metadata(id: &str, rec: &BookRecord) -> ScrapedMetadata {
    let mut mapped = Vec::new();
    let mut raw = Vec::new();

    for name in &rec.authors {
        raw.push(RawTag {
            namespace: "author".into(),
            value: name.clone(),
        });
        mapped.push(MappedTag {
            namespace: "creator".into(),
            value: name.clone(),
            qualifier: "none".into(),
            role: "none".into(),
        });
    }

    let mut tags = BTreeSet::new();
    for s in &rec.subjects {
        subject_tags(s, &mut tags);
    }
    for value in tags {
        raw.push(RawTag {
            namespace: "subject".into(),
            value: value.clone(),
        });
        mapped.push(MappedTag {
            namespace: "tag".into(),
            value,
            qualifier: "none".into(),
            role: "none".into(),
        });
    }

    let language = rec.language.as_deref().map(language_name);
    if let Some(lang) = &language {
        mapped.push(MappedTag {
            namespace: "language".into(),
            value: lang.clone(),
            qualifier: "none".into(),
            role: "none".into(),
        });
    }

    ScrapedMetadata {
        title: (!rec.title.is_empty()).then(|| rec.title.clone()),
        language,
        description: rec.summary.clone(),
        source_url: Some(source_url(id)),
        raw_tags: raw,
        mapped_tags: mapped,
        comments: Vec::new(),
        cover_url: rec.cover.clone().or_else(|| Some(cover_url(id))),
        page_count: None,
        favorites: rec.downloads,
        ..Default::default()
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::*;
    use arcagrad_plugin_sdk::{guest, HttpFetchRequest};
    use extism_pdk::*;

    fn get(url: &str) -> Result<String, Error> {
        let mut req = HttpFetchRequest::get(url);
        req.headers
            .insert("User-Agent".to_string(), USER_AGENT.to_string());
        let resp = guest::fetch(&req)?;
        if resp.status != 200 {
            return Err(Error::msg(format!(
                "Gutenberg GET {url} -> HTTP {}",
                resp.status
            )));
        }
        Ok(resp.body)
    }

    fn book(id: &str) -> Result<BookRecord, Error> {
        let body = get(&format!("https://www.gutenberg.org/ebooks/{id}.opds"))?;
        parse_book(&body).map_err(Error::msg)
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
        let req: BrowseRequest = serde_json::from_str(&input)?;
        let url = browse_url(&req.feed, req.query.as_deref(), req.page);
        let body = get(&url)?;
        let page = BrowsePage {
            items: parse_search(&body).map_err(Error::msg)?,
            num_pages: None,
        };
        Ok(serde_json::to_string(&page)?)
    }

    #[plugin_fn]
    pub fn fetch_details(input: String) -> FnResult<String> {
        let candidate: Candidate = serde_json::from_str(&input)?;
        let id = parse_book_id(&candidate.id)
            .ok_or_else(|| Error::msg("not a Gutenberg book id/URL"))?;
        let rec = book(&id)?;
        Ok(serde_json::to_string(&book_metadata(&id, &rec))?)
    }

    #[plugin_fn]
    pub fn download(input: String) -> FnResult<String> {
        let candidate: Candidate = serde_json::from_str(&input)?;
        let id = parse_book_id(&candidate.id)
            .ok_or_else(|| Error::msg("not a Gutenberg book id/URL"))?;
        let rec = book(&id)?;
        let url = rec
            .epub
            .clone()
            .ok_or_else(|| Error::msg("this book has no EPUB format on Gutenberg"))?;
        let filename = epub_filename(&rec.title);
        let plan = DownloadPlan {
            url,
            filename,
            metadata: book_metadata(&id, &rec),
            ..Default::default()
        };
        Ok(serde_json::to_string(&plan)?)
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

    const SEARCH_XML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:opds="http://opds-spec.org/2010/catalog" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:opensearch="http://a9.com/-/spec/opensearch/1.1/">
<id>http://www.gutenberg.org/ebooks/search.opds/?query=dickens</id>
<title>Books: dickens</title>
<opensearch:itemsPerPage>25</opensearch:itemsPerPage>
<opensearch:startIndex>1</opensearch:startIndex>
<link rel="next" title="Next Page" type="application/atom+xml;profile=opds-catalog" href="/ebooks/search.opds/?query=dickens&amp;start_index=26"/>
<entry>
<updated>2026-07-12T23:28:48Z</updated>
<id>https://www.gutenberg.org/ebooks/authors/search.opds/?query=dickens</id>
<title>Authors</title>
<content type="text">5 author names match your search.</content>
<link type="application/atom+xml;profile=opds-catalog" rel="subsection" href="/ebooks/authors/search.opds/?query=dickens"/>
</entry>
<entry>
<updated>2026-07-12T23:28:48Z</updated>
<id>https://www.gutenberg.org/ebooks/98.opds</id>
<title>A Tale of Two Cities</title>
<content type="text">Charles Dickens</content>
<link type="application/atom+xml;profile=opds-catalog" rel="subsection" href="/ebooks/98.opds"/>
</entry>
<entry>
<updated>2026-07-12T23:28:48Z</updated>
<id>https://www.gutenberg.org/ebooks/766.opds</id>
<title>David Copperfield &amp; Friends</title>
<content type="text">Charles Dickens</content>
<link type="application/atom+xml;profile=opds-catalog" rel="subsection" href="/ebooks/766.opds"/>
</entry>
</feed>"#;

    const BOOK_XML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:opds="http://opds-spec.org/2010/catalog" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:relevance="http://a9.com/-/opensearch/extensions/relevance/1.0/">
<id>http://www.gutenberg.org/ebooks/1342.opds</id>
<title>Pride and Prejudice by Jane Austen</title>
<entry>
<updated>2026-07-12T23:31:15Z</updated>
<title>Pride and Prejudice</title>
<content type="xhtml">
<div xmlns="http://www.w3.org/1999/xhtml">
<p>This edition had all images removed.</p>
<p>
Summary:
"Pride and Prejudice" by Jane Austen is a novel published in 1813. (This is an automatically generated summary.)
</p>
<p>Author: Austen, Jane, 1775-1817</p>
<p>EBook No.: 1342</p>
<p>Downloads: 139860</p>
<p>Language: English</p>
</div>
</content>
<id>urn:gutenberg:1342:2</id>
<author><name>Austen, Jane</name></author>
<category scheme="http://purl.org/dc/terms/LCSH" term="England -- Fiction"/>
<category scheme="http://purl.org/dc/terms/LCSH" term="Courtship -- Fiction"/>
<category scheme="http://purl.org/dc/terms/LCSH" term="Love stories"/>
<category scheme="http://purl.org/dc/terms/LCC" term="PR" label="Language and Literatures: English literature"/>
<dcterms:language>en</dcterms:language>
<link type="application/epub+zip" rel="http://opds-spec.org/acquisition" title="EPUB (no images, older E-readers)" length="558543" href="https://www.gutenberg.org/ebooks/1342.epub.noimages"/>
<link type="image/jpeg" rel="http://opds-spec.org/image" href="https://www.gutenberg.org/cache/epub/1342/pg1342.cover.medium.jpg"/>
</entry>
<entry>
<updated>2026-07-12T23:31:15Z</updated>
<title>Pride and Prejudice</title>
<id>urn:gutenberg:1342:3</id>
<author><name>Austen, Jane</name></author>
<dcterms:language>en</dcterms:language>
<link type="application/epub+zip" rel="http://opds-spec.org/acquisition" title="EPUB3 (E-readers incl. Send-to-Kindle)" href="https://www.gutenberg.org/ebooks/1342.epub3.images"/>
<link type="application/epub+zip" rel="http://opds-spec.org/acquisition" title="EPUB (older E-readers)" href="https://www.gutenberg.org/ebooks/1342.epub.images"/>
</entry>
</feed>"#;

    #[test]
    fn browse_url_selects_feed_sort_query_and_page() {
        assert_eq!(
            browse_url("popular", None, 1),
            "https://www.gutenberg.org/ebooks/search.opds/?sort_order=downloads"
        );
        assert_eq!(
            browse_url("recent", None, 1),
            "https://www.gutenberg.org/ebooks/search.opds/?sort_order=release_date"
        );
        assert_eq!(
            browse_url("popular", Some("jane austen"), 3),
            "https://www.gutenberg.org/ebooks/search.opds/?sort_order=downloads&query=jane%20austen&start_index=51"
        );
    }

    #[test]
    fn parse_search_skips_navigation_and_maps_books() {
        let items = parse_search(SEARCH_XML).unwrap();
        assert_eq!(items.len(), 2, "the Authors subsection must be skipped");
        assert_eq!(items[0].reference, "98");
        assert_eq!(items[0].title, "A Tale of Two Cities");
        assert_eq!(items[0].subtitle.as_deref(), Some("Charles Dickens"));
        assert_eq!(
            items[0].cover_url,
            "https://www.gutenberg.org/cache/epub/98/pg98.cover.medium.jpg"
        );
        assert_eq!(
            items[0].source_url.as_deref(),
            Some("https://www.gutenberg.org/ebooks/98")
        );
        assert_eq!(items[1].title, "David Copperfield & Friends");
    }

    #[test]
    fn parse_book_merges_editions_and_prefers_images_epub3() {
        let rec = parse_book(BOOK_XML).unwrap();
        assert_eq!(rec.title, "Pride and Prejudice");
        assert_eq!(rec.authors, vec!["Jane Austen".to_string()]);
        assert_eq!(rec.language.as_deref(), Some("en"));
        assert_eq!(rec.downloads, Some(139860));
        assert!(rec
            .summary
            .as_deref()
            .unwrap()
            .starts_with("\"Pride and Prejudice\" by Jane Austen"));
        assert_eq!(
            rec.epub.as_deref(),
            Some("https://www.gutenberg.org/ebooks/1342.epub3.images"),
            "epub3.images must win over epub.noimages"
        );
        assert_eq!(
            rec.cover.as_deref(),
            Some("https://www.gutenberg.org/cache/epub/1342/pg1342.cover.medium.jpg")
        );
        assert_eq!(
            rec.subjects.len(),
            3,
            "LCSH only: the LCC category is not a subject"
        );
    }

    #[test]
    fn book_metadata_maps_author_subjects_language_summary() {
        let rec = parse_book(BOOK_XML).unwrap();
        let m = book_metadata("1342", &rec);
        assert_eq!(m.title.as_deref(), Some("Pride and Prejudice"));
        assert_eq!(m.language.as_deref(), Some("english"));
        assert_eq!(m.favorites, Some(139860));
        assert!(m.description.is_some());
        assert_eq!(
            m.source_url.as_deref(),
            Some("https://www.gutenberg.org/ebooks/1342")
        );
        assert!(m.mapped_tags.contains(&MappedTag {
            namespace: "creator".into(),
            value: "Jane Austen".into(),
            qualifier: "none".into(),
            role: "none".into(),
        }));
        assert!(m
            .mapped_tags
            .iter()
            .any(|t| t.namespace == "tag" && t.value == "courtship"));
        assert!(!m.mapped_tags.iter().any(|t| t.value == "fiction"));
        assert!(m.mapped_tags.contains(&MappedTag {
            namespace: "language".into(),
            value: "english".into(),
            qualifier: "none".into(),
            role: "none".into(),
        }));
    }

    #[test]
    fn parse_search_truncates_padded_pages_to_the_stride() {
        let mut entries = String::new();
        for i in 1..=30 {
            entries.push_str(&format!(
                r#"<entry><id>https://www.gutenberg.org/ebooks/{i}.opds</id><title>Book {i}</title><content type="text">A. Author</content></entry>"#
            ));
        }
        let xml = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><feed xmlns="http://www.w3.org/2005/Atom">{entries}</feed>"#
        );
        let items = parse_search(&xml).unwrap();
        assert_eq!(items.len(), PAGE_SIZE as usize);
        assert_eq!(items[0].reference, "1");
        assert_eq!(items.last().unwrap().reference, "25");
    }

    #[test]
    fn parse_book_id_accepts_ids_and_urls() {
        assert_eq!(parse_book_id("1342").as_deref(), Some("1342"));
        assert_eq!(parse_book_id("  1342 ").as_deref(), Some("1342"));
        assert_eq!(
            parse_book_id("https://www.gutenberg.org/ebooks/1342").as_deref(),
            Some("1342")
        );
        assert_eq!(
            parse_book_id("https://www.gutenberg.org/ebooks/1342.opds").as_deref(),
            Some("1342")
        );
        assert_eq!(parse_book_id("not a book"), None);
    }

    #[test]
    fn format_author_and_filename() {
        assert_eq!(format_author("Austen, Jane"), "Jane Austen");
        assert_eq!(format_author("Various"), "Various");
        assert_eq!(
            epub_filename("Pride and Prejudice"),
            "Pride and Prejudice.epub"
        );
    }
}
