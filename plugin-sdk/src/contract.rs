//! Request and response types used across the WASM boundary.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// What the host knows about a local item, handed to a scraper to find a match.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScrapeHint {
    /// Best-known title (filename-derived today).
    pub title: String,
    /// Clean catalog title, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_title: Option<String>,
    /// Primary creator/author, when known from the item's `creator` tags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Local rendering modality, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modality: Option<String>,
    /// Page count, if known, a strong gallery disambiguator.
    pub page_count: Option<i64>,
    /// User-supplied source URL or id, treated as opaque by the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

/// A possible source match. `id` is opaque to the host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub title: String,
    /// 0.0–1.0 scraper-assigned confidence; lets the host pick the best.
    pub score: f32,
}

/// A source tag before namespace mapping.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RawTag {
    /// The source's own category label (e.g. `"artist"`, `"female"`).
    pub namespace: String,
    pub value: String,
}

/// A tag mapped into arcagrad's tag model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MappedTag {
    pub namespace: String,
    pub value: String,
    pub qualifier: String,
    #[serde(default = "facet_none")]
    pub role: String,
}

fn facet_none() -> String {
    "none".to_string()
}

/// A read-only comment mirrored from a source.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScrapedComment {
    pub external_id: String,
    pub author: String,
    #[serde(default)]
    pub posted_at: Option<i64>,
    #[serde(default)]
    pub score: Option<i64>,
    pub body: String,
}

/// The metadata a scraper resolved for a candidate.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScrapedMetadata {
    pub title: Option<String>,
    pub language: Option<String>,
    /// Source description. The client sanitizes markup before rendering it.
    #[serde(default)]
    pub description: Option<String>,
    /// Canonical URL for the source item.
    #[serde(default)]
    pub source_url: Option<String>,
    /// Source tags before mapping.
    pub raw_tags: Vec<RawTag>,
    /// Tags mapped into our namespaces plus qualifier (to `item_tags`, provenance = id).
    pub mapped_tags: Vec<MappedTag>,
    /// Comments returned by the source.
    #[serde(default)]
    pub comments: Vec<ScrapedComment>,
    /// Cover/thumbnail URL (resolved to a full CDN URL, which the client proxies).
    #[serde(default)]
    pub cover_url: Option<String>,
    /// Page count (the "N pages" badge).
    #[serde(default)]
    pub page_count: Option<i64>,
    /// Source popularity (favourites), a preview badge.
    #[serde(default)]
    pub favorites: Option<i64>,
    /// The series' chapters, for a browse preview of a chaptered manga source.
    #[serde(default)]
    pub chapters: Vec<ScrapedChapter>,
}

/// One remote chapter from a browse source: a display number, title, and page count.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScrapedChapter {
    #[serde(default)]
    pub number: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    pub page_count: i64,
    /// Opaque handle passed to the plugin's `pages` export.
    #[serde(default)]
    pub reference: Option<String>,
}

/// A linked series passed to a calendar plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarReference {
    pub reference: String,
    #[serde(default)]
    pub title: Option<String>,
}

/// A batched calendar lookup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarRequest {
    pub window_start: String,
    pub window_end: String,
    pub references: Vec<CalendarReference>,
    #[serde(default)]
    pub market: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarRelease {
    /// Stable source identity used to update an existing release.
    pub release_id: String,
    pub label: String,
    #[serde(default)]
    pub title: Option<String>,
    pub release_date: String,
    #[serde(default = "calendar_day_precision")]
    pub date_precision: String,
    #[serde(default = "calendar_announced_status")]
    pub date_status: String,
    #[serde(default)]
    pub formats: Vec<String>,
    #[serde(default)]
    pub media_type: Option<String>,
    #[serde(default)]
    pub market: Option<String>,
    #[serde(default)]
    pub publisher: Option<String>,
    #[serde(default)]
    pub creators: Vec<String>,
    #[serde(default)]
    pub isbn: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub cover_url: Option<String>,
}

fn calendar_day_precision() -> String {
    "day".to_string()
}

fn calendar_announced_status() -> String {
    "announced".to_string()
}

/// Releases returned for one requested reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarSeriesResult {
    pub reference: String,
    #[serde(default)]
    pub releases: Vec<CalendarRelease>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarReferenceError {
    pub reference: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CalendarResponse {
    #[serde(default)]
    pub results: Vec<CalendarSeriesResult>,
    #[serde(default)]
    pub errors: Vec<CalendarReferenceError>,
}

/// Instructions returned by a plugin's `download` export.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DownloadPlan {
    /// Direct, fetchable file URL (subject to the host's anti-SSRF guard).
    pub url: String,
    /// Suggested filename for the ingested archive (sanitized host-side).
    pub filename: String,
    /// Headers sent on the initial download request.
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    /// Metadata to apply to the ingested item (tags / source_url / comments).
    #[serde(default)]
    pub metadata: ScrapedMetadata,
}

/// A page request the host hands a plugin's `browse` export.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowseRequest {
    /// Which declared feed to fetch (a `Feed::id`).
    pub feed: String,
    /// Free-text filter, when the feed accepts one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Chosen time range (one of the feed's `Feed::ranges`), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<String>,
    /// 1-based page number.
    #[serde(default = "one")]
    pub page: u32,
}

fn one() -> u32 {
    1
}

/// One item in a browse result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct BrowseItem {
    pub reference: String,
    pub title: String,
    /// Remote cover/thumbnail URL on the source's CDN.
    pub cover_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_count: Option<i64>,
    /// Source popularity (e.g. a gallery source's favourites), an optional card badge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub favorites: Option<i64>,
    /// Source rating on a 0–10 scale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<f32>,
    /// Secondary title shown when it differs from `title`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    /// Canonical source URL used for exact ownership matching.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
}

/// A page of browse results.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct BrowsePage {
    pub items: Vec<BrowseItem>,
    /// Total pages the source reports for this feed, if known; drives the pager.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub num_pages: Option<i64>,
}

/// One remote page and its display dimensions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct BrowsePageInfo {
    /// 1-based page number.
    pub number: i64,
    /// Full-resolution image URL (reader).
    pub image_url: String,
    /// Thumbnail URL (grid).
    pub thumb_url: String,
    pub width: i64,
    pub height: i64,
}

/// The ordered page list for a browse item.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(utoipa::ToSchema))]
pub struct BrowsePages {
    pub pages: Vec<BrowsePageInfo>,
}

/// Input for reverse-image identification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentifyRequest {
    /// SHA-1 of the original first-page bytes.
    #[serde(default)]
    pub sha1: Option<String>,
    /// The item's page count — the plugin's primary re-ranking signal.
    #[serde(default)]
    pub page_count: Option<i64>,
    /// The item's clean title, a weak secondary hint.
    #[serde(default)]
    pub title_hint: Option<String>,
}

/// One reverse-image match.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentifyCandidate {
    /// Vendor id (e.g. "openlibrary") — matched against an installed plugin's `source`.
    pub source: String,
    /// The opaque per-vendor reference (e.g. anilist `"<gid>/<token>"`), fed to scrape.
    pub reference: String,
    #[serde(default)]
    pub title: Option<String>,
    /// Similarity score from 0 to 100.
    pub similarity: f32,
    /// Page count reported by the source.
    #[serde(default)]
    pub page_count: Option<i64>,
    /// External URL, for the degrade-to-link case when no plugin handles `source`.
    #[serde(default)]
    pub url: Option<String>,
    /// Source thumbnail URL.
    #[serde(default)]
    pub cover_url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IdentifyResult {
    pub candidates: Vec<IdentifyCandidate>,
}

fn default_method() -> String {
    "GET".to_string()
}

/// What a plugin passes to the `http_fetch` host function (as a JSON string).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpFetchRequest {
    #[serde(default = "default_method")]
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: String,
}

impl HttpFetchRequest {
    /// A plain GET with no extra headers, the common case.
    pub fn get(url: impl Into<String>) -> Self {
        HttpFetchRequest {
            method: "GET".to_string(),
            url: url.into(),
            headers: BTreeMap::new(),
            body: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Response from the host fetcher. Status `0` represents a network failure.
pub struct HttpFetchResponse {
    pub status: u16,
    pub body: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrape_hint_minimal_and_full_wire() {
        let h: ScrapeHint = serde_json::from_str(r#"{"title":"t","page_count":3}"#).unwrap();
        assert_eq!(h.title, "t");
        assert_eq!(h.page_count, Some(3));
        assert!(h.display_title.is_none() && h.author.is_none());
        assert!(h.modality.is_none() && h.reference.is_none());
        let json = serde_json::to_value(&h).unwrap();
        assert_eq!(json, serde_json::json!({"title": "t", "page_count": 3}));
    }

    #[test]
    fn scraped_metadata_minimal_wire() {
        let m: ScrapedMetadata = serde_json::from_str(
            r#"{"title":null,"language":"english","raw_tags":[],"mapped_tags":[{"namespace":"artist","value":"a","qualifier":"none"}]}"#,
        )
        .unwrap();
        assert_eq!(m.language.as_deref(), Some("english"));
        assert_eq!(m.mapped_tags.len(), 1);
        assert!(m.comments.is_empty() && m.chapters.is_empty());
        assert!(m.description.is_none() && m.cover_url.is_none());
    }

    #[test]
    fn download_plan_defaults_and_roundtrip() {
        let p: DownloadPlan =
            serde_json::from_str(r#"{"url":"https://x/f.cbz","filename":"f.cbz"}"#).unwrap();
        assert!(p.headers.is_empty());
        assert!(p.metadata.mapped_tags.is_empty());
        let p2 = DownloadPlan {
            headers: vec![("Cookie".into(), "x".into())],
            ..p
        };
        let json = serde_json::to_value(&p2).unwrap();
        assert_eq!(json["headers"], serde_json::json!([["Cookie", "x"]]));
        let back: DownloadPlan = serde_json::from_value(json).unwrap();
        assert_eq!(back.headers, p2.headers);
    }

    #[test]
    fn browse_request_page_defaults_to_one() {
        let r: BrowseRequest = serde_json::from_str(r#"{"feed":"popular"}"#).unwrap();
        assert_eq!(r.page, 1);
        assert!(r.query.is_none() && r.range.is_none());
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json, serde_json::json!({"feed": "popular", "page": 1}));
    }

    #[test]
    fn browse_item_optional_badges_are_omitted() {
        let item = BrowseItem {
            reference: "r".into(),
            title: "t".into(),
            cover_url: "c".into(),
            page_count: None,
            favorites: None,
            rating: None,
            subtitle: None,
            source_url: None,
        };
        let json = serde_json::to_value(&item).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"reference": "r", "title": "t", "cover_url": "c"})
        );
    }

    #[test]
    fn calendar_release_defaults() {
        let r: CalendarRelease = serde_json::from_str(
            r#"{"release_id":"i","label":"Vol 1","release_date":"2026-08-01"}"#,
        )
        .unwrap();
        assert_eq!(r.date_precision, "day");
        assert_eq!(r.date_status, "announced");
        assert!(r.formats.is_empty() && r.creators.is_empty());
    }

    #[test]
    fn http_fetch_request_method_defaults_to_get() {
        let r: HttpFetchRequest = serde_json::from_str(r#"{"url":"https://x/"}"#).unwrap();
        assert_eq!(r.method, "GET");
        assert!(r.headers.is_empty() && r.body.is_empty());
    }

    #[test]
    fn identify_request_fields_all_default() {
        let r: IdentifyRequest = serde_json::from_str("{}").unwrap();
        assert!(r.sha1.is_none() && r.page_count.is_none() && r.title_hint.is_none());
    }
}
