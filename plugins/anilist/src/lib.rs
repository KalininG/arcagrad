use std::collections::BTreeMap;

use serde::Deserialize;

const GRAPHQL_URL: &str = "https://graphql.anilist.co";
const USER_AGENT: &str = "arcagrad-anilist/0.1 (+https://github.com/arcagrad/arcagrad)";

use arcagrad_plugin_sdk::{
    Candidate, MappedTag, PluginManifest, RateLimit, RateRule, ReferenceInput, ScrapeHint,
    ScrapedMetadata, CONTRACT_VERSION, MANIFEST_VERSION,
};

fn manifest_doc() -> PluginManifest {
    PluginManifest {
        manifest_version: MANIFEST_VERSION,
        id: "anilist".into(),
        version: "0.1.0".into(),
        author: "KalininG".into(),
        icon: None,
        repository: Some("https://github.com/KalininG/arcagrad/tree/main/plugins/anilist".into()),
        name: "AniList".into(),
        description:
            "Manga/manhwa/manhua/light-novel metadata from AniList (genres, tags, staff, series URL)."
                .into(),
        source: "anilist".into(),
        capabilities: vec!["scrape".into()],
        hosts: vec!["anilist.co".into()],
        auth: None,
        rate_limit: Some(RateLimit {
            rules: vec![RateRule {
                match_pattern: String::new(),
                requests: 85,
                per_ms: 60_000,
            }],
            max_concurrency: 1,
        }),
        feeds: Vec::new(),
        reference_inputs: BTreeMap::from([(
            "scrape".into(),
            ReferenceInput {
                label: "AniList URL or media ID".into(),
                placeholder: "https://anilist.co/manga/34632/ or 34632".into(),
                help: "Optional. Selects an exact AniList entry instead of searching by title."
                    .into(),
                required: false,
            },
        )]),
        item_cache_ttl: 43200,
        image_headers: BTreeMap::new(),
        clean_titles: true,
        followable: true,
        reading_mode: "paged".into(),
        nsfw: false,
        contract_version: CONTRACT_VERSION,
    }
}

#[derive(Deserialize, Default, Clone)]
struct AlTitle {
    romaji: Option<String>,
    english: Option<String>,
}

#[derive(Deserialize)]
struct AlSearchMedia {
    id: i64,
    title: AlTitle,
}

#[derive(Deserialize)]
struct AlPage {
    #[serde(default)]
    media: Vec<AlSearchMedia>,
}

#[derive(Deserialize)]
struct AlSearchData {
    #[serde(rename = "Page")]
    page: AlPage,
}

#[derive(Deserialize)]
struct AlSearchEnvelope {
    data: Option<AlSearchData>,
}

#[derive(Deserialize)]
struct AlTag {
    name: String,
    category: Option<String>,
    #[serde(rename = "isGeneralSpoiler", default)]
    is_general_spoiler: bool,
    #[serde(default)]
    rank: i64,
}

#[derive(Deserialize)]
struct AlStaffName {
    full: String,
}

#[derive(Deserialize)]
struct AlStaffNode {
    name: AlStaffName,
}

#[derive(Deserialize)]
struct AlStaffEdge {
    role: String,
    node: AlStaffNode,
}

#[derive(Deserialize, Default)]
struct AlStaffConnection {
    #[serde(default)]
    edges: Vec<AlStaffEdge>,
}

#[derive(Deserialize)]
struct AlMediaDetail {
    title: AlTitle,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    genres: Vec<String>,
    #[serde(default)]
    tags: Vec<AlTag>,
    #[serde(default)]
    staff: AlStaffConnection,
    #[serde(rename = "siteUrl")]
    site_url: String,
}

#[derive(Deserialize)]
struct AlDetailData {
    #[serde(rename = "Media")]
    media: Option<AlMediaDetail>,
}

#[derive(Deserialize)]
struct AlDetailEnvelope {
    data: Option<AlDetailData>,
}

const SEARCH_QUERY: &str = "query($search:String!,$format:MediaFormat){Page(page:1,perPage:10){media(search:$search,type:MANGA,format:$format){id title{romaji english}}}}";
const DETAIL_QUERY: &str = "query($id:Int){Media(id:$id,type:MANGA){title{romaji english} description(asHtml:false) genres tags{name category isGeneralSpoiler rank} staff(sort:RELEVANCE){edges{role node{name{full}}}} siteUrl}}";

fn parse_reference(reference: &str) -> Option<i64> {
    let r = reference.trim();
    if let Ok(id) = r.parse::<i64>() {
        return Some(id);
    }
    let marker = "anilist.co/manga/";
    let idx = r.find(marker)?;
    let rest = &r[idx + marker.len()..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn pick_title(t: &AlTitle) -> Option<String> {
    t.english
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| t.romaji.clone().filter(|s| !s.is_empty()))
}

const MIN_TAG_RANK: i64 = 50;

const ARTIST_ROLES: &[&str] = &[
    "story & art",
    "story",
    "art",
    "original creator",
    "original story",
];

fn map_tags(genres: &[String], tags: &[AlTag]) -> Vec<MappedTag> {
    let mut out: Vec<MappedTag> = genres
        .iter()
        .filter(|g| !g.is_empty())
        .map(|g| MappedTag {
            namespace: "tag".to_string(),
            value: g.clone(),
            qualifier: "none".to_string(),
            role: "none".to_string(),
        })
        .collect();
    for t in tags {
        if t.is_general_spoiler || t.rank < MIN_TAG_RANK {
            continue;
        }
        let namespace = match &t.category {
            Some(c) if c.starts_with("Demographic") => "demographic",
            _ => "tag",
        };
        out.push(MappedTag {
            namespace: namespace.to_string(),
            value: t.name.clone(),
            qualifier: "none".to_string(),
            role: "none".to_string(),
        });
    }
    out
}

fn staff_role(r: &str) -> &'static str {
    match r.trim().to_lowercase().as_str() {
        "story" | "original story" => "writer",
        "art" => "illustrator",
        _ => "creator",
    }
}

fn map_staff(staff: &[AlStaffEdge]) -> Vec<MappedTag> {
    staff
        .iter()
        .filter(|e| ARTIST_ROLES.contains(&e.role.trim().to_lowercase().as_str()))
        .map(|e| MappedTag {
            namespace: "creator".to_string(),
            value: e.node.name.full.clone(),
            qualifier: "none".to_string(),
            role: staff_role(&e.role).to_string(),
        })
        .collect()
}

fn clean_description(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '<' {
            out.push(c);
            continue;
        }
        let mut tag = String::new();
        for c2 in chars.by_ref() {
            if c2 == '>' {
                break;
            }
            tag.push(c2);
        }
        let tag_lower = tag.trim_start_matches('/').trim().to_lowercase();
        if tag_lower == "br" || tag_lower.starts_with("br ") || tag_lower == "br/" {
            out.push('\n');
        }
    }
    // Limit blank-line runs introduced by adjacent break tags.
    let mut collapsed = String::with_capacity(out.len());
    let mut newline_run = 0;
    for c in out.chars() {
        if c == '\n' {
            newline_run += 1;
            if newline_run <= 2 {
                collapsed.push(c);
            }
        } else {
            newline_run = 0;
            collapsed.push(c);
        }
    }
    let trimmed = collapsed.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn metadata_from(detail: &AlMediaDetail) -> ScrapedMetadata {
    let mut mapped_tags = map_tags(&detail.genres, &detail.tags);
    mapped_tags.extend(map_staff(&detail.staff.edges));
    ScrapedMetadata {
        title: pick_title(&detail.title),
        language: None,
        source_url: Some(detail.site_url.clone()),
        description: detail.description.as_deref().and_then(clean_description),
        mapped_tags,
        ..Default::default()
    }
}

fn candidates_from(media: Vec<AlSearchMedia>) -> Vec<Candidate> {
    media
        .into_iter()
        .enumerate()
        .map(|(i, m)| Candidate {
            id: m.id.to_string(),
            title: pick_title(&m.title).unwrap_or_default(),
            score: (1.0 - i as f32 * 0.05).max(0.5),
        })
        .collect()
}

fn search_request_body(title: &str, reflowable: bool) -> String {
    // AniList treats an explicit null format as matching no format.
    let mut variables = serde_json::json!({ "search": title });
    if reflowable {
        variables["format"] = "NOVEL".into();
    }
    serde_json::json!({ "query": SEARCH_QUERY, "variables": variables }).to_string()
}

fn detail_request_body(id: i64) -> String {
    serde_json::json!({ "query": DETAIL_QUERY, "variables": { "id": id } }).to_string()
}

fn parse_search_response(body: &str) -> Vec<AlSearchMedia> {
    serde_json::from_str::<AlSearchEnvelope>(body)
        .ok()
        .and_then(|e| e.data)
        .map(|d| d.page.media)
        .unwrap_or_default()
}

fn parse_detail_response(body: &str) -> Option<AlMediaDetail> {
    serde_json::from_str::<AlDetailEnvelope>(body)
        .ok()
        .and_then(|e| e.data)
        .and_then(|d| d.media)
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::*;
    use arcagrad_plugin_sdk::{guest, HttpFetchRequest};
    use extism_pdk::*;

    fn do_post(body: String) -> Result<String, Error> {
        let mut req = HttpFetchRequest::get(GRAPHQL_URL);
        req.method = "POST".to_string();
        req.headers
            .insert("Content-Type".to_string(), "application/json".to_string());
        req.headers
            .insert("User-Agent".to_string(), USER_AGENT.to_string());
        req.body = body;
        let resp = guest::fetch(&req)?;
        if resp.status != 200 {
            return Err(Error::msg(format!("AniList HTTP {}", resp.status)));
        }
        Ok(resp.body)
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
        if let Some(id) = hint.reference.as_deref().and_then(parse_reference) {
            let only = vec![Candidate {
                id: id.to_string(),
                title: hint.title.clone(),
                score: 1.0,
            }];
            return Ok(serde_json::to_string(&only)?);
        }
        let reflowable = hint.modality.as_deref() == Some("reflowable");
        let body = do_post(search_request_body(&hint.title, reflowable))?;
        let media = parse_search_response(&body);
        Ok(serde_json::to_string(&candidates_from(media))?)
    }

    #[plugin_fn]
    pub fn fetch_details(input: String) -> FnResult<String> {
        let candidate: Candidate = serde_json::from_str(&input)?;
        let id: i64 = candidate
            .id
            .parse()
            .map_err(|_| Error::msg(format!("anilist: bad candidate id {:?}", candidate.id)))?;
        let body = do_post(detail_request_body(id))?;
        let detail = parse_detail_response(&body)
            .ok_or_else(|| Error::msg(format!("anilist: no manga with id {id}")))?;
        Ok(serde_json::to_string(&metadata_from(&detail))?)
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
    fn parses_bare_id() {
        assert_eq!(parse_reference("34632"), Some(34632));
        assert_eq!(parse_reference(" 34632 "), Some(34632));
    }

    #[test]
    fn parses_site_url_with_slug() {
        assert_eq!(
            parse_reference("https://anilist.co/manga/34632/Harbor-Lights"),
            Some(34632)
        );
    }

    #[test]
    fn parses_site_url_without_slug() {
        assert_eq!(
            parse_reference("https://anilist.co/manga/34632"),
            Some(34632)
        );
    }

    #[test]
    fn rejects_garbage_reference() {
        assert_eq!(parse_reference("not a reference"), None);
        assert_eq!(parse_reference(""), None);
    }

    #[test]
    fn reflowable_search_requests_novel_format_only() {
        let novel: serde_json::Value =
            serde_json::from_str(&search_request_body("Harbor Lights", true)).unwrap();
        assert_eq!(novel["variables"]["format"], "NOVEL");

        let manga: serde_json::Value =
            serde_json::from_str(&search_request_body("Harbor Lights", false)).unwrap();
        assert!(manga["variables"].get("format").is_none());
        assert!(manga["query"].as_str().unwrap().contains("type:MANGA"));
    }

    #[test]
    fn prefers_english_title() {
        let t = AlTitle {
            romaji: Some("Minato no Hikari".into()),
            english: Some("Harbor Lights".into()),
        };
        assert_eq!(pick_title(&t), Some("Harbor Lights".to_string()));
    }

    #[test]
    fn falls_back_to_romaji_when_no_english() {
        let t = AlTitle {
            romaji: Some("Minato no Hikari".into()),
            english: None,
        };
        assert_eq!(pick_title(&t), Some("Minato no Hikari".to_string()));
    }

    #[test]
    fn maps_genres_and_filters_tags() {
        let genres = vec!["Drama".to_string(), "Psychological".to_string()];
        let tags = vec![
            AlTag {
                name: "Seinen".into(),
                category: Some("Demographic".into()),
                is_general_spoiler: false,
                rank: 92,
            },
            AlTag {
                name: "Time Skip".into(),
                category: Some("Setting-Time".into()),
                is_general_spoiler: true,
                rank: 72,
            },
            AlTag {
                name: "Religion".into(),
                category: Some("Theme-Other".into()),
                is_general_spoiler: false,
                rank: 30,
            },
            AlTag {
                name: "Found Family".into(),
                category: Some("Theme-Other".into()),
                is_general_spoiler: false,
                rank: 61,
            },
        ];
        let mapped = map_tags(&genres, &tags);
        assert!(mapped.contains(&MappedTag {
            namespace: "tag".into(),
            value: "Drama".into(),
            qualifier: "none".into(),
            role: "none".into(),
        }));
        assert!(mapped.contains(&MappedTag {
            namespace: "demographic".into(),
            value: "Seinen".into(),
            qualifier: "none".into(),
            role: "none".into(),
        }));
        assert!(mapped.contains(&MappedTag {
            namespace: "tag".into(),
            value: "Found Family".into(),
            qualifier: "none".into(),
            role: "none".into(),
        }));
        assert!(!mapped.iter().any(|t| t.value == "Time Skip"));
        assert!(!mapped.iter().any(|t| t.value == "Religion"));
    }

    #[test]
    fn artist_role_allowlist_excludes_localization_credits() {
        let staff = vec![
            AlStaffEdge {
                role: "Story & Art".into(),
                node: AlStaffNode {
                    name: AlStaffName {
                        full: "Aiko Mori".into(),
                    },
                },
            },
            AlStaffEdge {
                role: "Touch-up Art & Lettering (English)".into(),
                node: AlStaffNode {
                    name: AlStaffName {
                        full: "Taylor Reed".into(),
                    },
                },
            },
            AlStaffEdge {
                role: "Translator (English)".into(),
                node: AlStaffNode {
                    name: AlStaffName {
                        full: "Northwind Localization".into(),
                    },
                },
            },
        ];
        let mapped = map_staff(&staff);
        assert_eq!(mapped.len(), 1);
        assert_eq!(mapped[0].value, "Aiko Mori");
        assert_eq!(mapped[0].namespace, "creator");
    }

    #[test]
    fn metadata_always_sets_source_url_to_site_url_and_skips_language() {
        let detail = AlMediaDetail {
            title: AlTitle {
                romaji: Some("Minato no Hikari".into()),
                english: Some("Harbor Lights".into()),
            },
            description: Some("Mika lives by the harbor.<br>She repairs old radios.".into()),
            genres: vec!["Drama".into()],
            tags: vec![],
            staff: AlStaffConnection { edges: vec![] },
            site_url: "https://anilist.co/manga/34632".into(),
        };
        let meta = metadata_from(&detail);
        assert_eq!(
            meta.source_url.as_deref(),
            Some("https://anilist.co/manga/34632")
        );
        assert_eq!(meta.title.as_deref(), Some("Harbor Lights"));
        assert!(meta.language.is_none());
        assert!(meta.comments.is_empty());
        assert_eq!(
            meta.description.as_deref(),
            Some("Mika lives by the harbor.\nShe repairs old radios.")
        );
    }

    #[test]
    fn cleans_anilist_description() {
        let raw = "Mika lives in a quiet seaside town.<br>\n\
                   She repairs old radios after school.<br>\n\
                   One evening, a distant signal appears.<br>\n\
                   Where could it be coming from\u{2026}?\n\
                   <br><br>\n\
                   (Source: Example Press)";
        let cleaned = clean_description(raw).unwrap();
        assert!(
            !cleaned.contains("<br>"),
            "HTML must be stripped: {cleaned}"
        );
        assert!(cleaned.contains("Mika lives in a quiet seaside town"));
        assert!(
            cleaned.contains("(Source: Example Press)"),
            "attribution is kept, not treated as noise"
        );
        assert!(cleaned.contains('\n'), "line breaks preserved as newlines");
    }

    #[test]
    fn clean_description_returns_none_for_empty_or_pure_markup() {
        assert_eq!(clean_description(""), None);
        assert_eq!(clean_description("<br><br>   "), None);
        assert_eq!(clean_description("   \n  "), None);
    }

    #[test]
    fn candidate_scores_are_positional_and_floored() {
        let media = vec![
            AlSearchMedia {
                id: 34632,
                title: AlTitle {
                    romaji: Some("Minato no Hikari".into()),
                    english: Some("Harbor Lights".into()),
                },
            },
            AlSearchMedia {
                id: 999,
                title: AlTitle {
                    romaji: Some("Something Else".into()),
                    english: None,
                },
            },
        ];
        let cands = candidates_from(media);
        assert_eq!(cands[0].id, "34632");
        assert_eq!(cands[0].score, 1.0);
        assert!(cands[1].score < cands[0].score);
    }

    #[test]
    fn parses_search_response() {
        let body = r#"{"data":{"Page":{"media":[{"id":34632,"title":{"romaji":"Minato no Hikari","english":"Harbor Lights"}}]}}}"#;
        let media = parse_search_response(body);
        assert_eq!(media.len(), 1);
        assert_eq!(media[0].id, 34632);
    }

    #[test]
    fn parse_detail_response_is_none_for_unknown_id() {
        let body = r#"{"data":{"Media":null}}"#;
        assert!(parse_detail_response(body).is_none());
    }
}
