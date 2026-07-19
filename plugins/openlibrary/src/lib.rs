use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use arcagrad_plugin_sdk::{
    Candidate, MappedTag, PluginManifest, RateLimit, RateRule, ReferenceInput, ScrapeHint,
    ScrapedMetadata, CONTRACT_VERSION, MANIFEST_VERSION,
};

const SEARCH_URL: &str = "https://openlibrary.org/search.json";
const USER_AGENT: &str = "arcagrad-openlibrary/0.1 (+https://github.com/arcagrad/arcagrad)";

fn manifest_doc() -> PluginManifest {
    PluginManifest {
        manifest_version: MANIFEST_VERSION,
        id: "openlibrary".into(),
        version: "0.1.0".into(),
        author: "KalininG".into(),
        icon: None,
        repository: Some(
            "https://github.com/KalininG/arcagrad/tree/main/plugins/openlibrary".into(),
        ),
        name: "Open Library".into(),
        description: "Metadata for books from Open Library subjects, settings, and time periods."
            .into(),
        source: "openlibrary".into(),
        capabilities: vec!["scrape".into()],
        hosts: vec!["openlibrary.org".into()],
        auth: None,
        rate_limit: Some(RateLimit {
            rules: vec![RateRule {
                match_pattern: String::new(),
                requests: 1,
                per_ms: 1_000,
            }],
            max_concurrency: 1,
        }),
        feeds: Vec::new(),
        reference_inputs: BTreeMap::from([(
            "scrape".into(),
            ReferenceInput {
                label: "Open Library URL, ID, or ISBN".into(),
                placeholder: "https://openlibrary.org/works/OL…W · OL…W · OL…M · ISBN".into(),
                help: "Optional. Paste an Open Library work or edition link/id, or an ISBN, to match a specific book; otherwise the title and author are used."
                    .into(),
                required: false,
            },
        )]),
        item_cache_ttl: 86400,
        image_headers: BTreeMap::new(),
        clean_titles: false,
        followable: true,
        reading_mode: "paged".into(),
        nsfw: false,
        contract_version: CONTRACT_VERSION,
    }
}

#[derive(Deserialize, Default)]
struct SearchEnvelope {
    #[serde(default)]
    docs: Vec<SearchDoc>,
}

#[derive(Deserialize)]
struct SearchDoc {
    key: String,
    title: String,
    #[serde(default)]
    author_name: Vec<String>,
}

#[derive(Deserialize)]
struct Work {
    key: String,
    #[serde(default)]
    subjects: Vec<String>,
    #[serde(default)]
    subject_places: Vec<String>,
    #[serde(default)]
    subject_times: Vec<String>,
}

fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn normalized(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn search_url(title: &str, author: &str) -> String {
    format!(
        "{SEARCH_URL}?title={}&author={}&limit=10&fields=key,title,author_name",
        percent_encode(title),
        percent_encode(author)
    )
}

#[derive(Debug, PartialEq)]
enum Reference {
    Work(String),
    Lookup(String),
}

fn parse_reference(reference: &str) -> Option<Reference> {
    let r = reference.trim();
    if r.is_empty() {
        return None;
    }
    // Isolate the work, edition, or ISBN token from URL and path forms.
    let tail = r.rsplit("/works/").next().unwrap_or(r);
    let tail = tail.rsplit("/books/").next().unwrap_or(tail);
    let token = tail.split(['/', '?', '#']).next().unwrap_or("").trim();
    if is_olid(token, b'W') {
        return Some(Reference::Work(format!("/works/{token}")));
    }
    if is_olid(token, b'M') {
        return Some(Reference::Lookup(format!(
            "https://openlibrary.org/books/{token}.json"
        )));
    }
    let isbn: String = token.chars().filter(|c| *c != '-').collect();
    if is_isbn(&isbn) {
        return Some(Reference::Lookup(format!(
            "https://openlibrary.org/isbn/{isbn}.json"
        )));
    }
    None
}

fn is_olid(token: &str, suffix: u8) -> bool {
    match token
        .strip_prefix("OL")
        .and_then(|s| s.strip_suffix(char::from(suffix)))
    {
        Some(digits) => !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()),
        None => false,
    }
}

fn is_isbn(t: &str) -> bool {
    match t.len() {
        13 => t.bytes().all(|b| b.is_ascii_digit()),
        10 => {
            t.bytes().take(9).all(|b| b.is_ascii_digit())
                && t.bytes()
                    .last()
                    .is_some_and(|b| b.is_ascii_digit() || b == b'X' || b == b'x')
        }
        _ => false,
    }
}

fn work_key_from_lookup(body: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Edition {
        #[serde(default)]
        works: Vec<WorkRef>,
    }
    #[derive(Deserialize)]
    struct WorkRef {
        key: String,
    }
    let edition: Edition = serde_json::from_str(body).ok()?;
    let key = edition.works.into_iter().next()?.key;
    let token = key.strip_prefix("/works/")?;
    is_olid(token, b'W').then_some(key)
}

fn candidates_from(body: &str, wanted_title: &str, wanted_author: &str) -> Vec<Candidate> {
    let Ok(envelope) = serde_json::from_str::<SearchEnvelope>(body) else {
        return Vec::new();
    };
    let wanted_title = normalized(wanted_title);
    let wanted_author = normalized(wanted_author);
    envelope
        .docs
        .into_iter()
        .filter_map(|doc| {
            let title = normalized(&doc.title);
            let author_matches = doc
                .author_name
                .iter()
                .any(|a| normalized(a) == wanted_author);
            if !author_matches {
                return None;
            }
            let score = if title == wanted_title {
                1.0
            } else if title.starts_with(&format!("{wanted_title} ")) {
                0.9
            } else {
                return None;
            };
            Some(Candidate {
                id: doc.key,
                title: doc.title,
                score,
            })
        })
        .collect()
}

fn is_call_number(lower: &str) -> bool {
    let first = lower.split_whitespace().next().unwrap_or("");
    // Dewey and UDC classification tokens.
    if is_dewey_token(first) {
        return true;
    }
    // Library of Congress classification tokens.
    if is_loc_token(first) {
        return true;
    }
    // Cutter-year suffixes catch remaining call-number forms.
    contains_cutter_year(lower)
}

fn is_dewey_token(token: &str) -> bool {
    let bytes = token.as_bytes();
    if bytes.is_empty()
        || !bytes
            .iter()
            .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'/'))
    {
        return false;
    }
    let leading = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
    let has_decimal = bytes.iter().any(|b| matches!(b, b'.' | b'/'));
    // Preserve short numbers and dates while rejecting classification decimals.
    leading >= 3 || (leading >= 2 && has_decimal)
}

fn is_loc_token(token: &str) -> bool {
    let letters = token.bytes().take_while(u8::is_ascii_alphabetic).count();
    (1..=3).contains(&letters)
        && token
            .as_bytes()
            .get(letters)
            .is_some_and(u8::is_ascii_digit)
}

fn contains_cutter_year(lower: &str) -> bool {
    let tokens: Vec<&str> = lower.split_whitespace().collect();
    tokens.windows(2).any(|w| is_cutter(w[0]) && is_year(w[1]))
}

fn is_cutter(token: &str) -> bool {
    let bytes = token.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1..].iter().all(u8::is_ascii_digit)
}

fn is_year(token: &str) -> bool {
    token.len() == 4
        && token.bytes().all(|b| b.is_ascii_digit())
        && token
            .parse::<u16>()
            .is_ok_and(|y| (1400..=2100).contains(&y))
}

fn strip_brackets(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_language(v: &str) -> bool {
    const LANGUAGES: &[&str] = &[
        "english",
        "spanish",
        "french",
        "german",
        "russian",
        "italian",
        "portuguese",
        "dutch",
        "japanese",
        "chinese",
        "korean",
        "arabic",
        "hebrew",
        "hindi",
        "polish",
        "swedish",
        "danish",
        "norwegian",
        "finnish",
        "czech",
        "turkish",
        "greek",
        "latin",
        "ukrainian",
        "romanian",
        "hungarian",
        "vietnamese",
        "thai",
        "indonesian",
        "persian",
        "bengali",
        "multilingual",
        "bilingual",
        "anglais",
        "francais",
        "deutsch",
        "espanol",
        "italiano",
        "russe",
        "eng",
        "spa",
        "fre",
        "fra",
        "ger",
        "deu",
        "rus",
        "ita",
        "por",
        "nld",
        "dut",
        "jpn",
        "zho",
        "chi",
        "kor",
        "ara",
        "heb",
        "hin",
        "pol",
        "swe",
        "dan",
        "nor",
        "fin",
        "ces",
        "cze",
        "tur",
        "ell",
        "grc",
        "ukr",
        "ron",
        "hun",
        "vie",
        "tha",
        "ind",
        "fas",
        "ben",
        "lat",
    ];
    LANGUAGES.contains(&v)
}

fn is_blocklisted_subject(v: &str) -> bool {
    const NON_SUBJECTS: &[&str] = &[
        "long now manual for civilization",
        "readers",
        "readers for new literates",
        "reading level-grade 8",
        "reading level-grade 9",
        "reading level-grade 10",
        "reading level-grade 11",
        "reading level-grade 12",
        "book",
        "books",
        "ebook",
        "ebooks",
        "audiobook",
        "audiobooks",
        "audio-book",
        "audio-books",
    ];
    NON_SUBJECTS.contains(&v)
}

fn normalize_subject(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let lower = raw.to_lowercase();
    if is_call_number(&lower) {
        return None;
    }
    // Remove catalog qualifiers and keep the head of LCSH subdivisions.
    let mut value = strip_brackets(&lower);
    if let Some(i) = value.find("--") {
        value.truncate(i);
    }
    let value = value.trim();
    if matches!(value, "fiction" | "fiction, general") {
        return None;
    }

    let mut value = value.to_string();
    if let Some(rest) = value.strip_prefix("fiction, ") {
        value = rest.to_string();
    }
    if let Some(rest) = value.strip_suffix(", fiction") {
        value = rest.to_string();
    }
    if let Some(rest) = value.strip_suffix(", general") {
        value = rest.to_string();
    }
    // Normalize MARC whitespace around punctuation.
    let value = value
        .replace("- ", "-")
        .replace(" -", "-")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if value.is_empty() || is_language(&value) || is_blocklisted_subject(&value) {
        return None;
    }
    Some(value)
}

fn metadata_from(work: Work) -> ScrapedMetadata {
    let mut values = BTreeSet::new();
    for subject in &work.subjects {
        if let Some(value) = normalize_subject(subject) {
            values.insert(value);
        }
    }
    for place in &work.subject_places {
        if let Some(value) = normalize_subject(place) {
            values.insert(value);
        }
    }
    for time in &work.subject_times {
        if let Some(value) = normalize_subject(time) {
            values.insert(value);
        }
    }

    // Preserve scalar metadata already embedded in the EPUB.
    ScrapedMetadata {
        title: None,
        language: None,
        description: None,
        source_url: Some(format!("https://openlibrary.org{}", work.key)),
        mapped_tags: values
            .into_iter()
            .map(|value| MappedTag {
                namespace: "tag".to_string(),
                value,
                qualifier: "none".to_string(),
                role: "none".to_string(),
            })
            .collect(),
        ..Default::default()
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::*;
    use arcagrad_plugin_sdk::{guest, HttpFetchRequest};
    use extism_pdk::*;

    fn get(url: String) -> Result<String, Error> {
        let mut req = HttpFetchRequest::get(url);
        req.headers
            .insert("User-Agent".to_string(), USER_AGENT.to_string());
        let response = guest::fetch(&req)?;
        if response.status != 200 {
            return Err(Error::msg(format!("Open Library HTTP {}", response.status)));
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
    pub fn search(input: String) -> FnResult<String> {
        let hint: ScrapeHint = serde_json::from_str(&input)?;
        // Do not turn a failed exact reference into a potentially unrelated search hit.
        if let Some(reference) = hint.reference.as_deref().and_then(parse_reference) {
            let key = match reference {
                Reference::Work(key) => key,
                Reference::Lookup(url) => {
                    let body = get(url)?;
                    work_key_from_lookup(&body)
                        .ok_or_else(|| Error::msg("Open Library edition/ISBN has no linked work"))?
                }
            };
            let title = hint.display_title.as_deref().unwrap_or(&hint.title);
            let only = vec![Candidate {
                id: key,
                title: title.to_string(),
                score: 1.0,
            }];
            return Ok(serde_json::to_string(&only)?);
        }
        let title = hint.display_title.as_deref().unwrap_or(&hint.title).trim();
        let author = hint.author.as_deref().unwrap_or_default().trim();
        if title.is_empty() || author.is_empty() {
            return Ok("[]".to_string());
        }
        let body = get(search_url(title, author))?;
        Ok(serde_json::to_string(&candidates_from(
            &body, title, author,
        ))?)
    }

    #[plugin_fn]
    pub fn fetch_details(input: String) -> FnResult<String> {
        let candidate: Candidate = serde_json::from_str(&input)?;
        if !candidate.id.starts_with("/works/OL") {
            return Err(Error::msg("Open Library candidate is not a work key").into());
        }
        let body = get(format!("https://openlibrary.org{}.json", candidate.id))?;
        let work: Work = serde_json::from_str(&body)?;
        Ok(serde_json::to_string(&metadata_from(work))?)
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

    #[test]
    fn exact_title_and_author_select_canonical_work_not_collections() {
        let body = r#"{"docs":[
          {"key":"/works/OL40879W","title":"Blood Meridian","author_name":["Cormac McCarthy"]},
          {"key":"/works/OL2W","title":"The Road / Blood Meridian / No Country for Old Men","author_name":["Cormac McCarthy"]},
          {"key":"/works/OL3W","title":"Blood Meridian","author_name":["Someone Else"]}
        ]}"#;
        let candidates = candidates_from(body, "Blood Meridian", "cormac mccarthy");
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].id, "/works/OL40879W");
        assert_eq!(candidates[0].score, 1.0);
    }

    #[test]
    fn normalizes_and_deduplicates_recommendation_tags() {
        let work: Work = serde_json::from_str(
            r#"{
              "key":"/works/OL40879W",
              "title":"Blood Meridian",
              "subjects":["Fiction","Outlaws","Fiction, westerns","Fiction, historical","Fiction, historical, general","Indians of North America, fiction"],
              "subject_places":["Mexican-American Border Region","Mexican- American Border Region"],
              "subject_times":["1833-1878"]
            }"#,
        )
        .unwrap();
        let metadata = metadata_from(work);
        let values: Vec<_> = metadata
            .mapped_tags
            .iter()
            .map(|t| t.value.as_str())
            .collect();
        assert!(!values.contains(&"fiction"));
        assert_eq!(values.iter().filter(|v| **v == "historical").count(), 1);
        assert!(values.contains(&"westerns"));
        assert!(values.contains(&"outlaws"));
        assert!(values.contains(&"indians of north america"));
        assert!(values.contains(&"1833-1878"));
        assert_eq!(
            values
                .iter()
                .filter(|v| **v == "mexican-american border region")
                .count(),
            1
        );
    }

    #[test]
    fn drops_library_call_numbers_but_keeps_time_periods() {
        for junk in [
            "823.912 f",
            "823/.912",
            "929/.343",
            "18.05 english literature",
            "pr6045.o72 t6 2005",
            "cs627.i74 t44 2007",
        ] {
            assert_eq!(normalize_subject(junk), None, "should drop `{junk}`");
        }
        assert_eq!(normalize_subject("9/11"), Some("9/11".to_string()));
        assert_eq!(
            normalize_subject("1833-1878"),
            Some("1833-1878".to_string())
        );
        assert_eq!(
            normalize_subject("Lighthouses"),
            Some("lighthouses".to_string())
        );
        assert_eq!(
            normalize_subject("Stream of consciousness fiction"),
            Some("stream of consciousness fiction".to_string())
        );
    }

    #[test]
    fn strips_bracketed_qualifiers_and_subdivisions() {
        assert_eq!(
            normalize_subject("Frankenstein (Fictional character)"),
            Some("frankenstein".to_string())
        );
        assert_eq!(
            normalize_subject("Frankenstein (Personaje literario)"),
            Some("frankenstein".to_string())
        );
        assert_eq!(
            normalize_subject("Geneva (Switzerland)"),
            Some("geneva".to_string())
        );
        assert_eq!(
            normalize_subject("Monsters--Fiction"),
            Some("monsters".to_string())
        );
        assert_eq!(
            normalize_subject("Frankenstein, Victor (Fictitious character)--Fiction"),
            Some("frankenstein, victor".to_string())
        );
    }

    #[test]
    fn drops_bare_language_tags_but_keeps_language_qualified_genres() {
        assert_eq!(normalize_subject("English"), None);
        assert_eq!(normalize_subject("Spanish"), None);
        assert_eq!(normalize_subject("eng"), None);
        assert_eq!(
            normalize_subject("English fiction"),
            Some("english fiction".to_string())
        );
        assert_eq!(
            normalize_subject("English literature"),
            Some("english literature".to_string())
        );
        assert_eq!(
            normalize_subject("Greek mythology"),
            Some("greek mythology".to_string())
        );
    }

    #[test]
    fn drops_blocklisted_user_list_names() {
        assert_eq!(normalize_subject("Long Now Manual for Civilization"), None);
    }

    #[test]
    fn drops_reader_audience_and_school_grade_cataloging() {
        for noise in [
            "Readers (Primary)",
            "Readers for new literates",
            "Reading level-grade 8",
            "Reading level-grade 9",
            "Reading level-grade 10",
            "Reading level-grade 11",
            "Reading level-grade 12",
        ] {
            assert_eq!(normalize_subject(noise), None, "should drop `{noise}`");
        }
        assert_eq!(
            normalize_subject("Grade inflation"),
            Some("grade inflation".to_string())
        );
    }

    #[test]
    fn drops_exact_generic_book_formats_without_substring_matching() {
        for noise in [
            "Book",
            "Books",
            "eBook",
            "eBooks",
            "Audiobook",
            "Audiobooks",
            "Audio-book",
            "Audio-books",
        ] {
            assert_eq!(normalize_subject(noise), None, "should drop `{noise}`");
        }

        for useful in ["Textbook", "Bookbinding", "Audiobook industry"] {
            assert_eq!(
                normalize_subject(useful),
                Some(useful.to_lowercase()),
                "should preserve `{useful}`"
            );
        }
    }

    #[test]
    fn bracketed_variants_collapse_to_one_deduped_tag() {
        let work: Work = serde_json::from_str(
            r#"{
              "key":"/works/OL450063W",
              "title":"Frankenstein",
              "subjects":[
                "Frankenstein (Fictional character)",
                "Frankenstein (Fictitious character)",
                "Frankenstein (Personaje literario)",
                "Monsters--Fiction",
                "Monsters",
                "English"
              ]
            }"#,
        )
        .unwrap();
        let metadata = metadata_from(work);
        let values: Vec<_> = metadata
            .mapped_tags
            .iter()
            .map(|t| t.value.as_str())
            .collect();
        assert_eq!(values.iter().filter(|v| **v == "frankenstein").count(), 1);
        assert_eq!(values.iter().filter(|v| **v == "monsters").count(), 1);
        assert!(!values.contains(&"english"));
    }

    #[test]
    fn parses_a_direct_work_reference_from_url_path_or_bare_id() {
        let key = Some(Reference::Work("/works/OL40879W".to_string()));
        assert_eq!(
            parse_reference("https://openlibrary.org/works/OL40879W"),
            key
        );
        assert_eq!(
            parse_reference("https://openlibrary.org/works/OL40879W/Blood_Meridian"),
            key
        );
        assert_eq!(parse_reference("/works/OL40879W"), key);
        assert_eq!(parse_reference("  OL40879W  "), key);
    }

    #[test]
    fn parses_an_edition_or_isbn_reference_into_a_lookup() {
        let lookup = Some(Reference::Lookup(
            "https://openlibrary.org/books/OL7353617M.json".to_string(),
        ));
        assert_eq!(parse_reference("OL7353617M"), lookup);
        assert_eq!(
            parse_reference("https://openlibrary.org/books/OL7353617M"),
            lookup
        );
        assert_eq!(
            parse_reference("https://openlibrary.org/books/OL7353617M/Moby-Dick"),
            lookup
        );
        assert_eq!(
            parse_reference("9780679728757"),
            Some(Reference::Lookup(
                "https://openlibrary.org/isbn/9780679728757.json".to_string()
            ))
        );
        assert_eq!(
            parse_reference("0-679-72875-4"),
            Some(Reference::Lookup(
                "https://openlibrary.org/isbn/0679728754.json".to_string()
            ))
        );
    }

    #[test]
    fn rejects_non_references_so_they_fall_back_to_search() {
        assert_eq!(parse_reference("not a reference"), None);
        assert_eq!(parse_reference("OL123"), None);
        assert_eq!(parse_reference("12345"), None);
        assert_eq!(parse_reference(""), None);
    }

    #[test]
    fn lookup_body_yields_the_linked_work_key_or_none() {
        assert_eq!(
            work_key_from_lookup(r#"{"works":[{"key":"/works/OL102749W"}]}"#),
            Some("/works/OL102749W".to_string())
        );
        assert_eq!(work_key_from_lookup(r#"{"works":[]}"#), None);
        assert_eq!(work_key_from_lookup(r#"{}"#), None);
        assert_eq!(
            work_key_from_lookup(r#"{"works":[{"key":"/authors/OL1A"}]}"#),
            None
        );
    }

    #[test]
    fn url_encodes_title_and_author() {
        let url = search_url("Blood Meridian", "Cormac McCarthy");
        assert!(url.contains("title=Blood%20Meridian"));
        assert!(url.contains("author=Cormac%20McCarthy"));
    }
}
