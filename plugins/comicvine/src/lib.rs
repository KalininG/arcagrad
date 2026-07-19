// Export-only code appears unused in host-target tests.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use std::collections::BTreeMap;

use serde::Deserialize;

use arcagrad_plugin_sdk::{
    AuthField, AuthSpec, Candidate, MappedTag, PluginManifest, RateLimit, RateRule, ReferenceInput,
    ScrapeHint, ScrapedMetadata, CONTRACT_VERSION, MANIFEST_VERSION,
};

const API: &str = "https://comicvine.gamespot.com/api";
// Comic Vine rejects generic user agents with an HTML 403.
const USER_AGENT: &str = "arcagrad-comicvine/0.1 (+https://github.com/arcagrad/arcagrad)";
const TYPE_ISSUE: &str = "4000";
const TYPE_VOLUME: &str = "4050";

fn manifest_doc() -> PluginManifest {
    PluginManifest {
        manifest_version: MANIFEST_VERSION,
        id: "comicvine".into(),
        version: "0.1.0".into(),
        author: "KalininG".into(),
        icon: None,
        repository: Some(
            "https://github.com/KalininG/arcagrad/tree/main/plugins/comicvine".into(),
        ),
        name: "Comic Vine".into(),
        description: "Western-comics metadata (publisher, creators, characters) from Comic Vine."
            .into(),
        source: "comicvine".into(),
        capabilities: vec!["scrape".into()],
        hosts: vec!["comicvine.gamespot.com".into()],
        auth: Some(AuthSpec {
            scheme: "api_key".into(),
            fields: vec![AuthField {
                name: "api_key".into(),
                label: Some("API key".into()),
                secret: true,
                required: true,
                help: Some(
                    "Your Comic Vine API key. Required — Comic Vine rejects every request without one."
                        .into(),
                ),
            }],
            required_for: vec!["scrape".into()],
            setup: Some(
                "Free API key from comicvine.gamespot.com/api — sign in and click 'Grab an API key'."
                    .into(),
            ),
        }),
        rate_limit: Some(RateLimit {
            rules: vec![RateRule {
                match_pattern: String::new(),
                requests: 1,
                per_ms: 10_000,
            }],
            max_concurrency: 1,
        }),
        feeds: Vec::new(),
        reference_inputs: BTreeMap::from([(
            "scrape".into(),
            ReferenceInput {
                label: "Comic Vine volume or issue URL".into(),
                placeholder: "https://comicvine.gamespot.com/…/4050-796/".into(),
                help: "Optional. A Comic Vine volume (series) or issue URL selects an exact match instead of searching by title."
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

#[derive(Deserialize)]
struct CvObj<T> {
    #[serde(default)]
    status_code: i64,
    #[serde(default)]
    results: Option<T>,
}

#[derive(Deserialize)]
struct CvList<T> {
    #[serde(default)]
    #[allow(dead_code)]
    status_code: i64,
    #[serde(default)]
    results: Vec<T>,
}

#[derive(Deserialize, Default)]
struct CvNamed {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    name: String,
}

#[derive(Deserialize, Default)]
struct CvPerson {
    #[serde(default)]
    name: String,
    #[serde(default)]
    role: String,
}

#[derive(Deserialize, Default)]
struct CvAggregate {
    #[serde(default)]
    name: String,
    #[serde(default)]
    count: Option<String>,
}

#[derive(Deserialize, Default)]
struct CvVolume {
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    deck: Option<String>,
    #[serde(default)]
    site_detail_url: Option<String>,
    #[serde(default)]
    publisher: Option<CvNamed>,
    #[serde(default)]
    characters: Vec<CvAggregate>,
    #[serde(default)]
    people: Vec<CvAggregate>,
    #[serde(default)]
    concepts: Vec<CvAggregate>,
    #[serde(default)]
    teams: Vec<CvAggregate>,
}

#[derive(Deserialize, Default)]
struct CvIssue {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    deck: Option<String>,
    #[serde(default)]
    site_detail_url: Option<String>,
    #[serde(default)]
    volume: Option<CvNamed>,
    #[serde(default)]
    person_credits: Vec<CvPerson>,
    #[serde(default)]
    character_credits: Vec<CvNamed>,
    #[serde(default)]
    team_credits: Vec<CvNamed>,
    #[serde(default)]
    concept_credits: Vec<CvNamed>,
}

#[derive(Deserialize, Default)]
struct CvVolumeHit {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    name: String,
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

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Issue,
    Volume,
}

fn parse_reference(reference: &str) -> Option<(Kind, i64)> {
    let r = reference.trim();
    for (marker, kind) in [(TYPE_ISSUE, Kind::Issue), (TYPE_VOLUME, Kind::Volume)] {
        let needle = format!("{marker}-");
        if let Some(idx) = r.find(&needle) {
            let rest = &r[idx + needle.len()..];
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(id) = digits.parse::<i64>() {
                return Some((kind, id));
            }
        }
    }
    r.parse::<i64>().ok().map(|id| (Kind::Volume, id))
}

fn handle(kind: Kind, id: i64) -> String {
    let t = match kind {
        Kind::Issue => TYPE_ISSUE,
        Kind::Volume => TYPE_VOLUME,
    };
    format!("{t}-{id}")
}

const NON_CREATIVE_ROLES: &[&str] = &[
    "editor",
    "editor in chief",
    "executive editor",
    "senior editor",
    "associate editor",
    "assistant editor",
    "translator",
];

fn maps_to_artist(role_field: &str) -> bool {
    let roles: Vec<String> = role_field
        .split(',')
        .map(|r| r.trim().to_ascii_lowercase())
        .filter(|r| !r.is_empty())
        .collect();
    if roles.is_empty() {
        return true;
    }
    roles
        .iter()
        .any(|r| !NON_CREATIVE_ROLES.contains(&r.as_str()))
}

fn push_tag(out: &mut Vec<MappedTag>, namespace: &str, value: &str) {
    let v = value.trim();
    if v.is_empty() {
        return;
    }
    if out
        .iter()
        .any(|t| t.namespace == namespace && t.value.eq_ignore_ascii_case(v))
    {
        return;
    }
    out.push(MappedTag {
        namespace: namespace.to_string(),
        value: v.to_string(),
        qualifier: "none".to_string(),
        role: "none".to_string(),
    });
}

fn map_issue_credits(
    persons: &[CvPerson],
    characters: &[CvNamed],
    teams: &[CvNamed],
    concepts: &[CvNamed],
    publisher: Option<&CvNamed>,
) -> Vec<MappedTag> {
    let mut out = Vec::new();
    for p in persons {
        if maps_to_artist(&p.role) {
            push_tag(&mut out, "creator", &p.name);
        }
    }
    for c in characters {
        push_tag(&mut out, "character", &c.name);
    }
    for t in teams {
        push_tag(&mut out, "character", &t.name);
    }
    for c in concepts {
        push_tag(&mut out, "tag", &c.name);
    }
    if let Some(p) = publisher {
        push_tag(&mut out, "group", &p.name);
    }
    out
}

// Bound the large aggregate lists returned for long-running series.
const MAX_CHARACTERS: usize = 15;
const MAX_PEOPLE: usize = 8;
const MAX_CONCEPTS: usize = 10;
const MAX_TEAMS: usize = 6;

fn agg_count(a: &CvAggregate) -> i64 {
    a.count
        .as_deref()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

fn top_names(items: &[CvAggregate], n: usize) -> Vec<&str> {
    let mut refs: Vec<&CvAggregate> = items.iter().collect();
    refs.sort_by_key(|a| std::cmp::Reverse(agg_count(a)));
    refs.into_iter().take(n).map(|a| a.name.as_str()).collect()
}

fn map_volume(v: &CvVolume) -> Vec<MappedTag> {
    let mut out = Vec::new();
    if let Some(p) = &v.publisher {
        push_tag(&mut out, "group", &p.name);
    }
    for name in top_names(&v.characters, MAX_CHARACTERS) {
        push_tag(&mut out, "character", name);
    }
    for name in top_names(&v.teams, MAX_TEAMS) {
        push_tag(&mut out, "character", name);
    }
    for name in top_names(&v.concepts, MAX_CONCEPTS) {
        push_tag(&mut out, "tag", name);
    }
    for name in top_names(&v.people, MAX_PEOPLE) {
        push_tag(&mut out, "creator", name);
    }
    out
}

fn pick_description(description: &Option<String>, deck: &Option<String>) -> Option<String> {
    description
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| deck.clone().filter(|s| !s.trim().is_empty()))
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::*;
    use arcagrad_plugin_sdk::{guest, HttpFetchRequest};
    use extism_pdk::*;

    fn api_key() -> Option<String> {
        let raw = guest::credentials().ok()?;
        let creds: BTreeMap<String, String> = serde_json::from_str(&raw).ok()?;
        creds.get("api_key").filter(|k| !k.is_empty()).cloned()
    }

    fn get(path: &str, query: &str) -> Result<String, Error> {
        let key = api_key().ok_or_else(|| Error::msg("Comic Vine API key not configured"))?;
        let sep = if query.is_empty() { "" } else { "&" };
        // Without the trailing slash Comic Vine returns an HTML redirect.
        let mut req = HttpFetchRequest::get(format!(
            "{API}/{path}/?api_key={key}&format=json{sep}{query}"
        ));
        req.headers
            .insert("User-Agent".to_string(), USER_AGENT.to_string());
        let resp = guest::fetch(&req)?;
        if resp.status != 200 {
            return Err(Error::msg(format!(
                "Comic Vine {path} -> HTTP {}",
                resp.status
            )));
        }
        Ok(resp.body)
    }

    // Volume aggregates and issue credits use different response fields.
    const VOLUME_FIELDS: &str =
        "id,name,publisher,description,deck,site_detail_url,characters,people,concepts,teams";
    const ISSUE_FIELDS: &str = "id,name,description,deck,site_detail_url,volume,person_credits,character_credits,team_credits,concept_credits";

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
        if let Some((kind, id)) = hint.reference.as_deref().and_then(parse_reference) {
            let candidates = vec![Candidate {
                id: handle(kind, id),
                title: hint.title.clone(),
                score: 1.0,
            }];
            return Ok(serde_json::to_string(&candidates)?);
        }
        let query = format!(
            "resources=volume&field_list=id,name&limit=10&query={}",
            enc(hint.title.trim())
        );
        let body = get("search", &query)?;
        let env: CvList<CvVolumeHit> = serde_json::from_str(&body)?;
        let candidates: Vec<Candidate> = env
            .results
            .into_iter()
            .filter(|v| v.id > 0 && !v.name.is_empty())
            .map(|v| Candidate {
                id: handle(Kind::Volume, v.id),
                title: v.name,
                score: 0.5,
            })
            .collect();
        Ok(serde_json::to_string(&candidates)?)
    }

    #[plugin_fn]
    pub fn fetch_details(input: String) -> FnResult<String> {
        let candidate: Candidate = serde_json::from_str(&input)?;
        let (kind, id) = parse_reference(&candidate.id)
            .ok_or_else(|| Error::msg("unrecognized Comic Vine candidate id"))?;
        let meta = match kind {
            Kind::Volume => fetch_volume(id)?,
            Kind::Issue => fetch_issue(id)?,
        };
        Ok(serde_json::to_string(&meta)?)
    }

    fn fetch_volume(id: i64) -> Result<ScrapedMetadata, Error> {
        let body = get(
            &format!("volume/{TYPE_VOLUME}-{id}"),
            &format!("field_list={VOLUME_FIELDS}"),
        )?;
        let env: CvObj<CvVolume> = serde_json::from_str(&body)?;
        let v = env.results.ok_or_else(|| {
            Error::msg(format!(
                "Comic Vine volume {id} not found (status {})",
                env.status_code
            ))
        })?;
        let mapped_tags = map_volume(&v);
        Ok(ScrapedMetadata {
            title: (!v.name.is_empty()).then(|| v.name.clone()),
            language: None,
            description: pick_description(&v.description, &v.deck),
            source_url: v.site_detail_url.clone(),
            mapped_tags,
            ..Default::default()
        })
    }

    fn fetch_issue(id: i64) -> Result<ScrapedMetadata, Error> {
        let body = get(
            &format!("issue/{TYPE_ISSUE}-{id}"),
            &format!("field_list={ISSUE_FIELDS}"),
        )?;
        let env: CvObj<CvIssue> = serde_json::from_str(&body)?;
        let i = env.results.ok_or_else(|| {
            Error::msg(format!(
                "Comic Vine issue {id} not found (status {})",
                env.status_code
            ))
        })?;
        let publisher = i
            .volume
            .as_ref()
            .and_then(|vol| publisher_of_volume(vol.id));
        let mapped_tags = map_issue_credits(
            &i.person_credits,
            &i.character_credits,
            &i.team_credits,
            &i.concept_credits,
            publisher.as_ref(),
        );
        let title = i.name.clone().filter(|s| !s.trim().is_empty()).or_else(|| {
            i.volume
                .as_ref()
                .map(|v| v.name.clone())
                .filter(|s| !s.is_empty())
        });
        Ok(ScrapedMetadata {
            title,
            language: None,
            description: pick_description(&i.description, &i.deck),
            source_url: i.site_detail_url.clone(),
            mapped_tags,
            ..Default::default()
        })
    }

    fn publisher_of_volume(volume_id: i64) -> Option<CvNamed> {
        let body = get(
            &format!("volume/{TYPE_VOLUME}-{volume_id}"),
            "field_list=publisher",
        )
        .ok()?;
        let env: CvObj<CvVolume> = serde_json::from_str(&body).ok()?;
        env.results.and_then(|v| v.publisher)
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
    fn parses_references() {
        assert_eq!(
            parse_reference("https://comicvine.gamespot.com/batman/4050-796/"),
            Some((Kind::Volume, 796))
        );
        assert_eq!(
            parse_reference("https://comicvine.gamespot.com/x/4000-12345/"),
            Some((Kind::Issue, 12345))
        );
        assert_eq!(parse_reference("4050-796"), Some((Kind::Volume, 796)));
        assert_eq!(parse_reference("4000-1"), Some((Kind::Issue, 1)));
        assert_eq!(parse_reference("796"), Some((Kind::Volume, 796)));
        assert_eq!(parse_reference("not-a-ref"), None);
        assert_eq!(handle(Kind::Volume, 796), "4050-796");
        assert_eq!(handle(Kind::Issue, 12345), "4000-12345");
    }

    #[test]
    fn creative_roles_map_to_artist_editors_do_not() {
        assert!(maps_to_artist("writer"));
        assert!(maps_to_artist("penciler, inker"));
        assert!(maps_to_artist("cover"));
        assert!(maps_to_artist(""));
        assert!(maps_to_artist("writer, editor"));
        assert!(!maps_to_artist("editor"));
        assert!(!maps_to_artist("translator"));
        assert!(!maps_to_artist("assistant editor, editor"));
    }

    #[test]
    fn maps_issue_credits_into_closed_namespaces() {
        let persons = vec![
            CvPerson {
                name: "Scott Snyder".into(),
                role: "writer".into(),
            },
            CvPerson {
                name: "Greg Capullo".into(),
                role: "penciler, cover".into(),
            },
            CvPerson {
                name: "Some Editor".into(),
                role: "editor".into(),
            },
            CvPerson {
                name: "Scott Snyder".into(),
                role: "writer, cover".into(),
            },
        ];
        let characters = vec![CvNamed {
            id: 1,
            name: "Batman".into(),
        }];
        let teams = vec![CvNamed {
            id: 2,
            name: "Justice League".into(),
        }];
        let concepts = vec![CvNamed {
            id: 3,
            name: "Time Travel".into(),
        }];
        let publisher = CvNamed {
            id: 4,
            name: "DC Comics".into(),
        };
        let tags = map_issue_credits(&persons, &characters, &teams, &concepts, Some(&publisher));
        let has = |ns: &str, v: &str| tags.iter().any(|t| t.namespace == ns && t.value == v);
        assert!(has("creator", "Scott Snyder"));
        assert!(has("creator", "Greg Capullo"));
        assert!(!has("creator", "Some Editor"), "pure editor skipped");
        assert!(has("character", "Batman"));
        assert!(has("character", "Justice League"), "team maps to character");
        assert!(has("tag", "Time Travel"), "concept maps to tag");
        assert!(has("group", "DC Comics"), "publisher maps to group");
        assert_eq!(tags.iter().filter(|t| t.value == "Scott Snyder").count(), 1);
        assert!(tags.iter().all(|t| t.qualifier == "none"));
    }

    #[test]
    fn description_prefers_full_over_deck() {
        assert_eq!(
            pick_description(&Some("<p>full</p>".into()), &Some("deck".into())).as_deref(),
            Some("<p>full</p>")
        );
        assert_eq!(
            pick_description(&None, &Some("deck".into())).as_deref(),
            Some("deck")
        );
        assert_eq!(pick_description(&Some("  ".into()), &None), None);
    }

    #[test]
    fn volume_maps_top_n_by_count() {
        let v = CvVolume {
            name: "Batman".into(),
            publisher: Some(CvNamed {
                id: 10,
                name: "DC Comics".into(),
            }),
            characters: vec![
                CvAggregate {
                    name: "Batman".into(),
                    count: Some("689".into()),
                },
                CvAggregate {
                    name: "Robin".into(),
                    count: Some("400".into()),
                },
                CvAggregate {
                    name: "One-Panel Cameo".into(),
                    count: Some("1".into()),
                },
            ],
            people: vec![CvAggregate {
                name: "Bob Kane".into(),
                count: Some("300".into()),
            }],
            concepts: vec![CvAggregate {
                name: "Superhero".into(),
                count: Some("500".into()),
            }],
            teams: vec![],
            ..Default::default()
        };
        let tags = map_volume(&v);
        let has = |ns: &str, val: &str| tags.iter().any(|t| t.namespace == ns && t.value == val);
        assert!(has("group", "DC Comics"), "publisher maps to group");
        assert!(has("character", "Batman") && has("character", "Robin"));
        assert!(
            has("creator", "Bob Kane"),
            "volume people map to artist (roleless)"
        );
        assert!(has("tag", "Superhero"), "concept maps to tag");
        let ordered = top_names(&v.characters, 15);
        assert_eq!(ordered[0], "Batman");
        assert_eq!(ordered[1], "Robin");
        assert!(tags.iter().all(|t| t.qualifier == "none"));
    }

    #[test]
    fn parses_the_real_envelope_shapes() {
        let obj: CvObj<CvVolume> = serde_json::from_str(
            r#"{"status_code":1,"error":"OK","results":{"name":"Batman",
                "publisher":{"id":10,"name":"DC Comics"},
                "characters":[{"id":1,"name":"Batman","count":"689"}],
                "people":[{"id":2,"name":"Bob Kane","count":"300"}],
                "concepts":[],"teams":[],
                "site_detail_url":"https://comicvine.gamespot.com/batman/4050-796/"}}"#,
        )
        .unwrap();
        let v = obj.results.unwrap();
        assert_eq!(v.name, "Batman");
        assert_eq!(v.publisher.unwrap().name, "DC Comics");
        assert_eq!(v.characters[0].name, "Batman");
        assert_eq!(agg_count(&v.characters[0]), 689);

        let iss: CvObj<CvIssue> = serde_json::from_str(
            r#"{"status_code":1,"results":{"name":null,"volume":{"id":796,"name":"Batman"},
                "person_credits":[{"name":"Alvin Schwartz","role":"writer"},{"name":"Dick Sprang","role":"penciler, inker"}],
                "character_credits":[{"id":1,"name":"Alfred Pennyworth"}],"team_credits":[],"concept_credits":[]}}"#,
        )
        .unwrap();
        let i = iss.results.unwrap();
        assert_eq!(i.name, None);
        assert_eq!(i.person_credits[1].role, "penciler, inker");
        assert_eq!(i.volume.unwrap().name, "Batman");

        let list: CvList<CvVolumeHit> = serde_json::from_str(
            r#"{"status_code":1,"results":[{"id":796,"name":"Batman","resource_type":"volume"}]}"#,
        )
        .unwrap();
        assert_eq!(list.results[0].id, 796);
    }
}
