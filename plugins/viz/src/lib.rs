use std::collections::{BTreeMap, HashSet};

use regex::Regex;

use arcagrad_plugin_sdk::{
    CalendarRelease, CalendarRequest, PluginManifest, RateLimit, RateRule, ReferenceInput,
    CONTRACT_VERSION, MANIFEST_VERSION,
};

const VIZ_ORIGIN: &str = "https://www.viz.com";

fn manifest_doc() -> PluginManifest {
    PluginManifest {
        manifest_version: MANIFEST_VERSION,
        id: "viz".into(),
        version: "0.1.0".into(),
        author: "KalininG".into(),
        icon: None,
        repository: Some("https://github.com/KalininG/arcagrad/tree/main/plugins/viz".into()),
        name: "VIZ".into(),
        description: "Track announced manga and book releases directly from VIZ series pages."
            .into(),
        source: "viz".into(),
        capabilities: vec!["calendar".into()],
        hosts: vec!["viz.com".into(), "dw9to29mmj727.cloudfront.net".into()],
        auth: None,
        rate_limit: Some(RateLimit {
            rules: vec![RateRule {
                match_pattern: String::new(),
                requests: 1,
                per_ms: 1000,
            }],
            max_concurrency: 1,
        }),
        feeds: Vec::new(),
        reference_inputs: BTreeMap::from([(
            "calendar".into(),
            ReferenceInput {
                label: "VIZ series URL".into(),
                placeholder: "https://www.viz.com/spy-x-family".into(),
                help: "Paste a VIZ series page or Shonen Jump chapters URL. Arcaserver will check the canonical series page for upcoming releases."
                    .into(),
                required: true,
            },
        )]),
        item_cache_ttl: 43200,
        image_headers: BTreeMap::new(),
        clean_titles: false,
        followable: false,
        reading_mode: "paged".into(),
        nsfw: false,
        contract_version: CONTRACT_VERSION,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreorderProduct {
    url: String,
    title: String,
}

fn decode_html(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
}

fn text_content(value: &str) -> String {
    let tags = Regex::new(r"(?s)<[^>]*>").unwrap();
    decode_html(&tags.replace_all(value, " "))
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn direct_series_url(value: &str) -> Option<String> {
    let path = value
        .trim()
        .strip_prefix("https://www.viz.com/")
        .or_else(|| value.trim().strip_prefix("https://viz.com/"))
        .or_else(|| value.trim().strip_prefix("http://www.viz.com/"))
        .or_else(|| value.trim().strip_prefix("http://viz.com/"))?;
    let path = path
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .trim_end_matches('/');
    let slug = path.strip_prefix("shonenjump/chapters/").unwrap_or(path);
    if slug.is_empty()
        || slug.contains('/')
        || matches!(
            slug,
            "calendar"
                | "search"
                | "manga-books"
                | "shonenjump"
                | "vizmanga"
                | "anime"
                | "blog"
                | "apps"
                | "events"
        )
    {
        return None;
    }
    Some(format!("{VIZ_ORIGIN}/{slug}"))
}

fn preorder_products(html: &str) -> Vec<PreorderProduct> {
    let article = Regex::new(r"(?is)<article\b.*?</article>").unwrap();
    let product =
        Regex::new(r#"(?is)<a[^>]+href="(/manga-books/[^"?#]+/product/(\d+))"[^>]*>([^<]+)</a>"#)
            .unwrap();
    let mut seen = HashSet::new();
    let mut products = Vec::new();
    for block in article.find_iter(html).map(|m| m.as_str()) {
        if !block.to_ascii_lowercase().contains("pre-order") {
            continue;
        }
        if let Some(cap) = product.captures_iter(block).last() {
            let url = format!("{VIZ_ORIGIN}{}", &cap[1]);
            if seen.insert(url.clone()) {
                products.push(PreorderProduct {
                    url,
                    title: decode_html(cap[3].trim()),
                });
            }
        }
    }
    products
}

fn date_yyyy_mm_dd(value: &str) -> Option<String> {
    let date = Regex::new(r"(?i)^\s*([a-z]+)\s+(\d{1,2}),\s*(\d{4})\s*$").unwrap();
    let cap = date.captures(value)?;
    let month = match cap[1].to_ascii_lowercase().as_str() {
        "january" => 1,
        "february" => 2,
        "march" => 3,
        "april" => 4,
        "may" => 5,
        "june" => 6,
        "july" => 7,
        "august" => 8,
        "september" => 9,
        "october" => 10,
        "november" => 11,
        "december" => 12,
        _ => return None,
    };
    let day: u8 = cap[2].parse().ok()?;
    let year: u16 = cap[3].parse().ok()?;
    (1..=31)
        .contains(&day)
        .then(|| format!("{year:04}-{month:02}-{day:02}"))
}

fn capture_text(html: &str, pattern: &str) -> Option<String> {
    Regex::new(pattern)
        .ok()?
        .captures(html)
        .map(|cap| text_content(&cap[1]))
        .filter(|value| !value.is_empty())
}

fn title_and_label(value: &str) -> (String, String) {
    let suffix = Regex::new(r"(?i),?\s+(Vol(?:ume)?\.?\s+\d+(?:\.\d+)?)\s*$").unwrap();
    let label = suffix
        .captures(value)
        .map(|cap| cap[1].trim().to_string())
        .unwrap_or_else(|| "New release".to_string());
    let title = suffix.replace(value, "").trim().to_string();
    (title, label)
}

fn parse_product(
    html: &str,
    product: &PreorderProduct,
    request: &CalendarRequest,
) -> Result<Option<CalendarRelease>, String> {
    let raw_date = capture_text(
        html,
        r#"(?is)<div[^>]*class="[^"]*\bo_release-date\b[^"]*"[^>]*>\s*<strong>Release</strong>(.*?)</div>"#,
    )
    .ok_or_else(|| format!("VIZ product has no release date: {}", product.url))?;
    let release_date = date_yyyy_mm_dd(&raw_date)
        .ok_or_else(|| format!("VIZ returned an unknown release date `{raw_date}`"))?;
    if release_date < request.window_start || release_date > request.window_end {
        return Ok(None);
    }

    let og_title = capture_text(
        html,
        r#"(?is)<meta\s+property="og:title"\s+content="VIZ:\s*(?:See\s+)?([^"]+)"\s*/?>"#,
    )
    .unwrap_or_else(|| product.title.clone());
    let (title, label) = title_and_label(&og_title);
    let cover_url = capture_text(
        html,
        r#"(?is)<meta\s+property="og:image"\s+content="(https://dw9to29mmj727\.cloudfront\.net/[^"]+)"\s*/?>"#,
    );
    let isbn = capture_text(
        html,
        r#"(?is)<div[^>]*class="[^"]*\bo_isbn13\b[^"]*"[^>]*>\s*<strong>ISBN-13</strong>(.*?)</div>"#,
    );
    let creator_line = capture_text(
        html,
        r#"(?is)<div[^>]*class="[^"]*mar-b-md[^"]*"[^>]*>\s*<strong>[^<]*\bby</strong>(.*?)</div>"#,
    );
    let creators = creator_line
        .map(|line| {
            line.split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let media_type = capture_text(html, r#"(?is)<strong>Category</strong>\s*(.*?)</div>"#)
        .or_else(|| Some("Manga".to_string()));

    let mut formats = Vec::new();
    if html.contains("id=\"buy_paperback_tab\"")
        || html.contains("id=\"buy_hardcover_tab\"")
        || html.contains(">Paperback</a>")
        || html.contains(">Hardcover</a>")
    {
        formats.push("Print".to_string());
    }
    if html.contains("id=\"buy_digital_tab\"") || html.contains(">Digital</a>") {
        formats.push("Digital".to_string());
    }
    if formats.is_empty() {
        formats.push("Print".to_string());
    }

    let product_id = product
        .url
        .split("/product/")
        .nth(1)
        .and_then(|tail| tail.split('/').next())
        .unwrap_or(&product.url);
    Ok(Some(CalendarRelease {
        release_id: format!("viz:{product_id}"),
        label,
        title: Some(title),
        release_date,
        date_precision: "day".to_string(),
        date_status: "announced".to_string(),
        formats,
        media_type,
        market: request.market.clone().or_else(|| Some("en-US".to_string())),
        publisher: Some("VIZ Media".to_string()),
        creators,
        isbn,
        url: Some(product.url.clone()),
        cover_url,
    }))
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::*;
    use arcagrad_plugin_sdk::{
        guest, CalendarReferenceError, CalendarResponse, CalendarSeriesResult, HttpFetchRequest,
    };
    use extism_pdk::*;

    fn get(url: String) -> Result<String, Error> {
        let response = guest::fetch(&HttpFetchRequest::get(url))?;
        if response.status != 200 {
            return Err(Error::msg(format!("VIZ HTTP {}", response.status)));
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
    pub fn upcoming(input: String) -> FnResult<String> {
        let request: CalendarRequest = serde_json::from_str(&input)?;
        let mut results = Vec::new();
        let mut errors = Vec::new();

        for reference in &request.references {
            let Some(series_url) = direct_series_url(&reference.reference) else {
                errors.push(CalendarReferenceError {
                    reference: reference.reference.clone(),
                    message: "The linked reference is not a canonical VIZ series URL".to_string(),
                });
                continue;
            };
            let series_html = match get(series_url) {
                Ok(html) => html,
                Err(error) => {
                    errors.push(CalendarReferenceError {
                        reference: reference.reference.clone(),
                        message: error.to_string(),
                    });
                    continue;
                }
            };
            let mut releases = Vec::new();
            let mut failure = None;
            for product in preorder_products(&series_html) {
                match get(product.url.clone())
                    .map_err(|error| error.to_string())
                    .and_then(|html| parse_product(&html, &product, &request))
                {
                    Ok(Some(release)) => releases.push(release),
                    Ok(None) => {}
                    Err(error) => {
                        failure = Some(error);
                        break;
                    }
                }
            }
            if let Some(message) = failure {
                errors.push(CalendarReferenceError {
                    reference: reference.reference.clone(),
                    message,
                });
            } else {
                releases.sort_by(|a, b| {
                    a.release_date
                        .cmp(&b.release_date)
                        .then_with(|| a.label.cmp(&b.label))
                });
                results.push(CalendarSeriesResult {
                    reference: reference.reference.clone(),
                    releases,
                });
            }
        }

        Ok(serde_json::to_string(&CalendarResponse {
            results,
            errors,
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

    const SERIES_HTML: &str = r#"
      <div class="property-row"><a href="/hima-ten" rel="Hima-Ten!" class="disp-bl o_property-link">Hima-Ten!</a></div>
      <article><span class="product-tag">Pre-Order</span><a href="/manga-books/manga/hima-ten-volume-2-0/product/8980" class="product-thumb"></a><a href="/manga-books/manga/hima-ten-volume-2-0/product/8980">Hima-Ten!, Vol. 2</a></article>
      <article><a href="/manga-books/manga/hima-ten-volume-1-0/product/8900">Hima-Ten!, Vol. 1</a></article>
    "#;

    const PRODUCT_HTML: &str = r#"
      <meta property="og:title" content="VIZ: See Hima-Ten!, Vol. 2">
      <meta property="og:image" content="https://dw9to29mmj727.cloudfront.net/products/1974764435.jpg">
      <a id="buy_paperback_tab">Paperback</a><a id="buy_digital_tab">Digital</a>
      <div class="mar-b-md"><strong>Story and Art by</strong> Genki Ono</div>
      <div class="o_release-date mar-b-md"><strong>Release</strong> September  1, 2026</div>
      <div class="o_isbn13 mar-b-md"><strong>ISBN-13</strong> 978-1-9747-6443-3</div>
      <div><strong>Category</strong> Manga</div>
    "#;

    #[test]
    fn normalizes_supported_viz_series_urls() {
        assert_eq!(
            direct_series_url("http://viz.com/hima-ten/"),
            Some("https://www.viz.com/hima-ten".into())
        );
        assert_eq!(
            direct_series_url("https://www.viz.com/shonenjump/chapters/chainsaw-man"),
            Some("https://www.viz.com/chainsaw-man".into())
        );
        assert_eq!(
            direct_series_url(
                "https://www.viz.com/shonenjump/chapters/chainsaw-man/?source=search#chapters"
            ),
            Some("https://www.viz.com/chainsaw-man".into())
        );
        assert_eq!(direct_series_url("https://www.viz.com/calendar"), None);
        assert_eq!(
            direct_series_url("https://www.viz.com/manga-books/manga/hima-ten"),
            None
        );
        assert_eq!(
            direct_series_url("https://www.viz.com/shonenjump/chapters/chainsaw-man/42"),
            None
        );
    }

    #[test]
    fn follows_only_preorders_on_the_authoritative_series_page() {
        assert_eq!(
            preorder_products(SERIES_HTML),
            vec![PreorderProduct {
                url: "https://www.viz.com/manga-books/manga/hima-ten-volume-2-0/product/8980"
                    .into(),
                title: "Hima-Ten!, Vol. 2".into(),
            }]
        );
    }

    #[test]
    fn parses_viz_product_metadata() {
        let request = CalendarRequest {
            window_start: "2026-07-01".into(),
            window_end: "2026-10-01".into(),
            references: Vec::new(),
            market: Some("en-US".into()),
        };
        let product = preorder_products(SERIES_HTML).remove(0);
        let release = parse_product(PRODUCT_HTML, &product, &request)
            .unwrap()
            .unwrap();
        assert_eq!(release.release_id, "viz:8980");
        assert_eq!(release.title.as_deref(), Some("Hima-Ten!"));
        assert_eq!(release.label, "Vol. 2");
        assert_eq!(release.release_date, "2026-09-01");
        assert_eq!(release.formats, ["Print", "Digital"]);
        assert_eq!(release.creators, ["Genki Ono"]);
        assert_eq!(release.isbn.as_deref(), Some("978-1-9747-6443-3"));
        assert_eq!(
            release.cover_url.as_deref(),
            Some("https://dw9to29mmj727.cloudfront.net/products/1974764435.jpg")
        );
    }
}
