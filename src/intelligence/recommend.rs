//! Content-based recommendation scoring and caches.
//!
//! SQL candidate selection lives in `repo`; this module reranks candidates with
//! IDF-weighted tag vectors. Creator and group tags are handled as a flat bonus rather
//! than cosine dimensions so they do not overwhelm thematic matches.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

use lru::LruCache;

// ---- tunables ----

/// Relative weight of each tag namespace.
fn facet_boost(namespace: &str, value: &str) -> f32 {
    match namespace {
        "creator" => 3.0,
        // Groups often duplicate creator attribution.
        "group" => 0.25,
        // "Original" is a catch-all rather than a meaningful shared source.
        "parody" if is_original(value) => 0.5,
        "parody" => 1.0,
        "character" => 1.5,
        "tag" => 1.0,
        "category" | "demographic" => 0.5,
        "language" => 0.0,
        _ => 1.0,
    }
}

fn is_original(value: &str) -> bool {
    let v = value.trim();
    v.eq_ignore_ascii_case("original") || v.eq_ignore_ascii_case("original work")
}

const CREATOR_BONUS: f32 = 0.2;

fn with_creator_bonus(relevance: f32, shares_creator: bool) -> f32 {
    let bonus = if shares_creator { CREATOR_BONUS } else { 0.0 };
    (relevance + bonus).min(1.0)
}

const MIN_DF: i64 = 2;
const MAX_IDF: f32 = 3.0;
const TASTE_PER_CREATOR_CAP: usize = 3;
const SHRINK_K: f32 = 2.0;
const SHRINK_FULL: f32 = 8.0;

/// Evidence multiplier, normalized to reach 1 at `SHRINK_FULL`.
fn shrink_factor(shared: u32) -> f32 {
    let shared = shared as f32;
    let raw = shared / (shared + SHRINK_K);
    let at_full = SHRINK_FULL / (SHRINK_FULL + SHRINK_K);
    (raw / at_full).min(1.0)
}

/// Maximum neighbors stored per item.
pub const NEIGHBOR_CAP: usize = 50;
/// Maximum SQL candidates reranked per item.
pub const CANDIDATE_LIMIT: i64 = 500;
pub const FAVORITE_WEIGHT: f32 = 2.0;
/// Recency half-life in days for liked items.
pub const TASTE_HALF_LIFE_DAYS: f32 = 150.0;

/// `(tag_id, namespace, value, document_frequency)`.
pub type TagMeta = (i64, String, String, i64);

/// Clamped `log(N / df)`; zero means the tag is ignored.
fn idf(df: i64, n: i64) -> f32 {
    if df < MIN_DF || n <= 0 || df <= 0 {
        return 0.0;
    }
    ((n as f32) / (df as f32)).ln().clamp(0.0, MAX_IDF)
}

/// Candidate seeds must occur on at most 5% of items, with a floor of 200.
pub fn df_ceiling(n: i64) -> i64 {
    (n / 20).max(200)
}

/// Tags selective enough to seed candidates but common enough to have matches.
pub fn distinctive_tags(target: &[TagMeta], n: i64) -> Vec<i64> {
    let ceiling = df_ceiling(n);
    target
        .iter()
        .filter(|(_, _, _, df)| *df >= MIN_DF && *df <= ceiling)
        .map(|(id, _, _, _)| *id)
        .collect()
}

/// Precomputed tag weights and namespace metadata for a corpus.
pub struct Scorer {
    weights: HashMap<i64, f32>,
    namespace: HashMap<i64, String>,
    /// Creator and group tags excluded from cosine scoring.
    attribution_ids: HashSet<i64>,
}

impl Scorer {
    pub fn build(metas: &[TagMeta], n: i64) -> Self {
        let mut weights = HashMap::with_capacity(metas.len());
        let mut namespace = HashMap::with_capacity(metas.len());
        let mut attribution_ids = HashSet::new();
        for (id, ns, value, df) in metas {
            let w = idf(*df, n) * facet_boost(ns, value);
            if w > 0.0 {
                weights.insert(*id, w);
            }
            namespace.insert(*id, ns.clone());
            if ns == "creator" || ns == "group" {
                attribution_ids.insert(*id);
            }
        }
        Scorer {
            weights,
            namespace,
            attribution_ids,
        }
    }

    /// Squared L2 norm excluding creator and group tags.
    fn norm_sq(&self, tags: &[i64]) -> f32 {
        tags.iter()
            .filter(|t| !self.attribution_ids.contains(t))
            .filter_map(|t| self.weights.get(t))
            .map(|w| w * w)
            .sum()
    }

    /// Returns the dot product, shared-tag count, and candidate norm.
    fn overlap(&self, target: &HashSet<i64>, cand: &[i64]) -> (f32, u32, f32) {
        let (mut dot, mut shared, mut norm_c_sq) = (0.0, 0u32, 0.0);
        for t in cand {
            if self.attribution_ids.contains(t) {
                continue;
            }
            if let Some(&w) = self.weights.get(t) {
                norm_c_sq += w * w;
                if target.contains(t) {
                    dot += w * w;
                    shared += 1;
                }
            }
        }
        (dot, shared, norm_c_sq)
    }

    /// Whether a candidate shares any creator or group tag with a reference set.
    fn shares_attribution(&self, cand: &[i64], present: impl Fn(i64) -> bool) -> bool {
        cand.iter()
            .any(|&t| self.attribution_ids.contains(&t) && present(t))
    }

    /// IDF-weighted cosine of two thematic tag sets.
    pub fn cosine(&self, a: &[i64], b: &[i64]) -> f32 {
        let aset: HashSet<i64> = a.iter().copied().collect();
        let (dot, _, nb_sq) = self.overlap(&aset, b);
        let (na, nb) = (self.norm_sq(a).sqrt(), nb_sq.sqrt());
        if na == 0.0 || nb == 0.0 {
            0.0
        } else {
            dot / (na * nb)
        }
    }

    /// Primary creator/group key used by the taste-vector cap.
    fn attribution(&self, tags: &[i64]) -> Option<i64> {
        tags.iter()
            .find(|t| {
                self.namespace
                    .get(*t)
                    .map(|n| n == "creator")
                    .unwrap_or(false)
            })
            .or_else(|| {
                tags.iter().find(|t| {
                    self.namespace
                        .get(*t)
                        .map(|n| n == "group")
                        .unwrap_or(false)
                })
            })
            .copied()
    }

    /// Reranks candidates by adjusted cosine similarity.
    pub fn rerank(&self, target: &[i64], candidates: Vec<(i64, Vec<i64>)>) -> Vec<(i64, f32)> {
        let target_set: HashSet<i64> = target.iter().copied().collect();
        // Do not return early for a zero norm: attribution-only matches are still valid.
        let norm_t = self.norm_sq(target).sqrt();
        let mut scored: Vec<(i64, f32)> = candidates
            .into_iter()
            .map(|(id, tags)| {
                let (dot, shared, nc_sq) = self.overlap(&target_set, &tags);
                let cosine = if norm_t > 0.0 && nc_sq > 0.0 {
                    dot / (norm_t * nc_sq.sqrt())
                } else {
                    0.0
                };
                let relevance = cosine * shrink_factor(shared);
                let shares = self.shares_attribution(&tags, |t| target_set.contains(&t));
                (id, with_creator_bonus(relevance, shares))
            })
            .filter(|(_, score)| *score > 0.0)
            .collect();
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
        scored.truncate(NEIGHBOR_CAP);
        scored
    }

    // ---- "For You" (global personalized) ----

    /// Builds a normalized taste vector from weighted liked items.
    pub fn taste_vector(
        &self,
        liked: &[(i64, f32)],
        vectors: &HashMap<i64, Vec<i64>>,
    ) -> HashMap<i64, f32> {
        // Apply the per-creator cap after sorting by signal strength.
        let mut order: Vec<&(i64, f32)> = liked.iter().collect();
        order.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut per_creator: HashMap<i64, usize> = HashMap::new();
        let mut acc: HashMap<i64, f32> = HashMap::new();
        let mut total_w = 0.0f32;
        for &(item_id, w) in order {
            let Some(tags) = vectors.get(&item_id) else {
                continue;
            };
            if w <= 0.0 {
                continue;
            }
            // Empty contributions do not consume a creator slot.
            let contrib: Vec<(i64, f32)> = tags
                .iter()
                .filter_map(|t| self.weights.get(t).map(|&tw| (*t, tw)))
                .collect();
            if contrib.is_empty() {
                continue;
            }
            if let Some(a) = self.attribution(tags) {
                let c = per_creator.entry(a).or_insert(0);
                if *c >= TASTE_PER_CREATOR_CAP {
                    continue;
                }
                *c += 1;
            }
            total_w += w;
            for (t, tw) in contrib {
                *acc.entry(t).or_insert(0.0) += w * tw;
            }
        }
        if total_w == 0.0 {
            return HashMap::new();
        }
        for v in acc.values_mut() {
            *v /= total_w;
        }
        let norm = acc.values().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in acc.values_mut() {
                *v /= norm;
            }
        }
        acc
    }

    /// Thematic dot product against a taste vector. Attribution is added separately.
    pub fn score_against_taste(&self, taste: &HashMap<i64, f32>, candidate: &[i64]) -> f32 {
        candidate
            .iter()
            .filter(|t| !self.attribution_ids.contains(t))
            .filter_map(|t| Some(taste.get(t)? * self.weights.get(t)?))
            .sum()
    }

    /// Ranks personalized candidates relative to the strongest thematic match.
    pub fn rank_for_you(
        &self,
        taste: &HashMap<i64, f32>,
        candidates: Vec<(i64, Vec<i64>)>,
    ) -> Vec<(i64, f32)> {
        let scored: Vec<(i64, f32, bool)> = candidates
            .into_iter()
            .map(|(id, tags)| {
                let dot = self.score_against_taste(taste, &tags);
                let liked_creator = self.shares_attribution(&tags, |t| taste.contains_key(&t));
                (id, dot, liked_creator)
            })
            .filter(|(_, dot, liked_creator)| *dot > 0.0 || *liked_creator)
            .collect();
        // Normalize before adding the attribution bonus.
        let max = scored.iter().map(|(_, d, _)| *d).fold(0.0f32, f32::max);
        let mut ranked: Vec<(i64, f32)> = scored
            .into_iter()
            .map(|(id, dot, liked_creator)| {
                let rel = if max > 0.0 { dot / max } else { 0.0 };
                (id, with_creator_bonus(rel, liked_creator))
            })
            .collect();
        ranked.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
        ranked.truncate(NEIGHBOR_CAP);
        ranked
    }
}

/// Corpus-wide scorer and document frequencies, shared across recommendation requests.
pub struct Corpus {
    scorer: Scorer,
    df: HashMap<i64, i64>,
    n: i64,
}

impl Corpus {
    /// Builds a corpus from tag metadata and the item count.
    pub fn from_metas(metas: &[TagMeta], n: i64) -> Self {
        let df = metas.iter().map(|(id, _, _, df)| (*id, *df)).collect();
        Corpus {
            scorer: Scorer::build(metas, n),
            df,
            n,
        }
    }

    pub fn scorer(&self) -> &Scorer {
        &self.scorer
    }

    /// Returns the item's candidate-seeding tags.
    pub fn distinctive(&self, tag_ids: &[i64]) -> Vec<i64> {
        let ceiling = df_ceiling(self.n);
        tag_ids
            .iter()
            .copied()
            .filter(|t| matches!(self.df.get(t), Some(&df) if df >= MIN_DF && df <= ceiling))
            .collect()
    }
}

// ---- caches ----

/// A ranked neighbour list: `(item_id, similarity score)`, best-first.
pub type Neighbors = Arc<Vec<(i64, f32)>>;

/// LRU cache for per-item and per-user recommendation lists.
pub struct RecommendationCache<K: Eq + Hash> {
    inner: Mutex<LruCache<K, Neighbors>>,
}

impl<K: Eq + Hash> RecommendationCache<K> {
    pub fn new(capacity: usize) -> Self {
        let cap = NonZeroUsize::new(capacity).unwrap_or(NonZeroUsize::new(1).unwrap());
        RecommendationCache {
            inner: Mutex::new(LruCache::new(cap)),
        }
    }
    pub fn get(&self, key: &K) -> Option<Neighbors> {
        self.inner.lock().unwrap().get(key).cloned()
    }
    pub fn put(&self, key: K, value: Neighbors) {
        self.inner.lock().unwrap().put(key, value);
    }
    pub fn invalidate(&self, key: &K) {
        self.inner.lock().unwrap().pop(key);
    }
    pub fn clear(&self) {
        self.inner.lock().unwrap().clear();
    }
}

/// Cached corpus shared across requests.
#[derive(Default)]
pub struct CorpusCache {
    inner: Mutex<Option<Arc<Corpus>>>,
}

impl CorpusCache {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn get(&self) -> Option<Arc<Corpus>> {
        self.inner.lock().unwrap().clone()
    }
    pub fn set(&self, corpus: Arc<Corpus>) {
        *self.inner.lock().unwrap() = Some(corpus);
    }
    pub fn clear(&self) {
        *self.inner.lock().unwrap() = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: i64, ns: &str, df: i64) -> TagMeta {
        (id, ns.to_string(), String::new(), df)
    }
    fn meta_v(id: i64, ns: &str, value: &str, df: i64) -> TagMeta {
        (id, ns.to_string(), value.to_string(), df)
    }

    #[test]
    fn idf_rewards_rarity_and_clamps() {
        assert_eq!(idf(1, 1000), 0.0, "df<MIN_DF dropped");
        assert!(idf(2, 1000) > idf(100, 1000), "rarer tag scores higher");
        assert!(idf(2, 10_000_000) <= MAX_IDF, "clamped");
    }

    #[test]
    fn idf_cap_flattens_distinctive_tags_to_the_facet_prior() {
        let n = 10_000;
        let metas = vec![
            meta_v(1, "creator", "rare", 5),
            meta_v(2, "creator", "prolific", 300),
            meta_v(3, "parody", "pokemon", 50),
            meta_v(4, "parody", "obscure", 5),
            meta_v(5, "tag", "distinctive", 50),
            meta_v(6, "tag", "common", 6000),
        ];
        let s = Scorer::build(&metas, n);
        let w = |id| *s.weights.get(&id).unwrap();

        assert!(
            (w(1) - w(2)).abs() < 1e-4,
            "creator weight flat across rarity"
        );
        assert!(
            (w(3) - w(4)).abs() < 1e-4,
            "parody weight flat across rarity"
        );
        assert!((w(1) - 3.0 * MAX_IDF).abs() < 1e-4, "creator = boost×cap");
        assert!((w(3) - 1.0 * MAX_IDF).abs() < 1e-4, "parody = boost×cap");
        assert!(
            (w(5) - 1.0 * MAX_IDF).abs() < 1e-4,
            "distinctive content = boost×cap"
        );
        assert!(w(6) < w(5), "ubiquitous content demoted below distinctive");
        assert!(
            (w(1) / w(5) - 3.0).abs() < 1e-4,
            "creator:content is the 3:1 prior, not a blowup"
        );
    }

    #[test]
    fn distinctive_drops_unique_and_ultra_common() {
        let n = 1000;
        let target = vec![
            meta(1, "creator", 5),
            meta(2, "tag", 1),
            meta(3, "parody", 900),
            meta(4, "tag", 80),
        ];
        let mut d = distinctive_tags(&target, n);
        d.sort();
        assert_eq!(d, vec![1, 4]);
    }

    #[test]
    fn cosine_ranks_and_is_bounded() {
        let n = 1000;
        let metas = vec![meta(1, "tag", 10), meta(2, "tag", 10), meta(3, "tag", 10)];
        let s = Scorer::build(&metas, n);
        assert!(
            (s.cosine(&[1, 2, 3], &[1, 2, 3]) - 1.0).abs() < 1e-5,
            "identical is 1"
        );
        assert_eq!(s.cosine(&[1], &[2]), 0.0, "disjoint is 0");
        let partial = s.cosine(&[1, 2], &[2, 3]);
        assert!(partial > 0.0 && partial < 1.0, "partial overlap in (0,1)");
    }

    #[test]
    fn shared_creator_adds_a_flat_bonus() {
        let n = 1000;
        let metas = vec![meta(1, "creator", 10), meta(9, "tag", 10)];
        let s = Scorer::build(&metas, n);
        let target = vec![1, 9];
        let theme_only = (100, vec![9]);
        let theme_and_creator = (200, vec![9, 1]);
        let ranked = s.rerank(&target, vec![theme_only, theme_and_creator]);
        let score = |id| ranked.iter().find(|(i, _)| *i == id).unwrap().1;
        assert!(
            score(200) > score(100),
            "sharing the creator too ranks higher"
        );
        assert!(
            (score(200) - score(100) - CREATOR_BONUS).abs() < 1e-4,
            "the shared creator adds exactly a flat bonus, not a norm-distorting dimension"
        );
    }

    #[test]
    fn parody_original_gets_a_reduced_boost() {
        let n = 1000;
        let real = meta_v(1, "parody", "pokemon", 10);
        let orig = meta_v(2, "parody", "original", 10);
        let content = meta_v(3, "tag", "drama", 10);
        let s = Scorer::build(&[real, orig, content], n);
        let w = |id| *s.weights.get(&id).unwrap();
        assert!(w(1) > w(2), "real parody ({}) > original ({})", w(1), w(2));
        assert!(
            w(2) < w(3),
            "original ({}) < a content tag ({})",
            w(2),
            w(3)
        );
        assert!(
            (w(2) / w(1) - 0.5 / 1.0).abs() < 1e-5,
            "original boost is 0.5 vs 1.0"
        );
    }

    #[test]
    fn attribution_bonus_covers_creator_or_group_once() {
        let n = 1000;
        let metas = vec![
            meta(1, "creator", 10),
            meta(2, "group", 10),
            meta(9, "tag", 10),
        ];
        let s = Scorer::build(&metas, n);
        let target = vec![1, 2, 9];
        let theme = (10, vec![9]);
        let theme_group = (20, vec![9, 2]);
        let theme_both = (30, vec![9, 1, 2]);
        let ranked = s.rerank(&target, vec![theme, theme_group, theme_both]);
        let score = |id| ranked.iter().find(|(i, _)| *i == id).unwrap().1;
        assert!(
            (score(20) - score(10) - CREATOR_BONUS).abs() < 1e-4,
            "a shared group is attribution too"
        );
        assert!(
            (score(30) - score(20)).abs() < 1e-4,
            "creator + group is one attribution hit, not two"
        );
    }

    #[test]
    fn shrink_demotes_thin_overlap() {
        let n = 1000;
        let metas = vec![
            meta(1, "tag", 10),
            meta(2, "tag", 10),
            meta(3, "tag", 10),
            meta(4, "tag", 10),
        ];
        let s = Scorer::build(&metas, n);
        let target = vec![1, 2, 3];
        let thin = (10, vec![1, 4]);
        let rich = (20, vec![1, 2, 3]);
        let ranked = s.rerank(&target, vec![thin, rich]);
        assert_eq!(ranked[0].0, 20, "the richer overlap ranks first");
        let score = |id| ranked.iter().find(|(i, _)| *i == id).unwrap().1;
        assert!(
            score(20) > 2.0 * score(10),
            "thin overlap shrunk well below rich: rich={}, thin={}",
            score(20),
            score(10)
        );
    }

    #[test]
    fn shrink_factor_ramps_in_then_caps() {
        assert!(
            (shrink_factor(10) - 1.0).abs() < 1e-6,
            "no shrink at the cap"
        );
        assert!(
            (shrink_factor(25) - 1.0).abs() < 1e-6,
            "clamped to 1.0 above the cap"
        );
        assert!(
            shrink_factor(1) > 0.35 && shrink_factor(1) < 0.45,
            "1 shared tag ≈ 0.40, got {}",
            shrink_factor(1)
        );
        assert!(shrink_factor(1) < shrink_factor(3));
        assert!(shrink_factor(3) < shrink_factor(7));
        assert!(
            shrink_factor(7) < 1.0,
            "just under the cap is still shy of full"
        );
    }

    #[test]
    fn well_tagged_exact_copy_scores_a_true_1_0() {
        let n = 1000;
        let metas: Vec<_> = (1..=10).map(|i| meta(i, "tag", 10)).collect();
        let s = Scorer::build(&metas, n);
        let target: Vec<i64> = (1..=10).collect();
        let ranked = s.rerank(&target, vec![(20, target.clone())]);
        assert!(
            (ranked[0].1 - 1.0).abs() < 1e-6,
            "≥cap shared tags → exact copy = 1.0, got {}",
            ranked[0].1
        );
    }

    #[test]
    fn language_is_not_a_similarity_factor() {
        let n = 1000;
        let metas = vec![meta(1, "language", 10), meta(2, "tag", 10)];
        let s = Scorer::build(&metas, n);
        assert!(
            s.rerank(&[1], vec![(10, vec![1])]).is_empty(),
            "language alone yields no similarity"
        );
        assert_eq!(s.rerank(&[2], vec![(20, vec![2])]).len(), 1);
    }

    #[test]
    fn taste_cap_ignores_zero_contribution_items() {
        let n = 1000;
        let metas = vec![
            meta(1, "creator", 1),
            meta(3, "tag", 10),
            meta(4, "tag", 10),
            meta(5, "tag", 10),
        ];
        let s = Scorer::build(&metas, n);
        let vectors: HashMap<i64, Vec<i64>> = HashMap::from([
            (100, vec![1, 3]),
            (101, vec![1, 4]),
            (102, vec![1, 5]),
            (103, vec![1]),
        ]);
        let liked = vec![(103, 100.0), (100, 10.0), (101, 9.0), (102, 8.0)];
        let taste = s.taste_vector(&liked, &vectors);
        assert!(
            taste.contains_key(&3) && taste.contains_key(&4) && taste.contains_key(&5),
            "a no-op like must not evict a real one from the per-creator cap"
        );
    }

    #[test]
    fn taste_vector_is_unit_normed_and_per_creator_capped() {
        let n = 1000;
        let metas = vec![
            meta(1, "creator", 10),
            meta(3, "tag", 10),
            meta(4, "tag", 10),
            meta(5, "tag", 10),
            meta(6, "tag", 10),
            meta(7, "tag", 10),
        ];
        let s = Scorer::build(&metas, n);
        let vectors: HashMap<i64, Vec<i64>> = HashMap::from([
            (100, vec![1, 3]),
            (101, vec![1, 4]),
            (102, vec![1, 5]),
            (103, vec![1, 6]),
            (104, vec![1, 7]),
        ]);
        let liked = vec![(100, 5.0), (101, 4.0), (102, 3.0), (103, 2.0), (104, 1.0)];
        let taste = s.taste_vector(&liked, &vectors);

        let norm: f32 = taste.values().map(|v| v * v).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-4,
            "taste is L2-normalized, norm={norm}"
        );
        assert!(taste.contains_key(&3) && taste.contains_key(&4) && taste.contains_key(&5));
        assert!(
            !taste.contains_key(&6) && !taste.contains_key(&7),
            "capped-out items don't contribute"
        );
    }

    #[test]
    fn for_you_ranks_by_theme_with_a_flat_creator_bonus() {
        let n = 1000;
        let metas = vec![
            meta(1, "creator", 10),
            meta(2, "creator", 10),
            meta(3, "tag", 10),
            meta(4, "tag", 10),
        ];
        let s = Scorer::build(&metas, n);
        let taste = s.taste_vector(&[(100, 1.0)], &HashMap::from([(100, vec![1, 3, 4])]));
        let strong_theme = (10, vec![3, 4]);
        let weak_theme_creator = (20, vec![3, 1]);
        let off_taste = (30, vec![2]);
        let ranked = s.rank_for_you(&taste, vec![off_taste, weak_theme_creator, strong_theme]);
        let score = |id| ranked.iter().find(|(i, _)| *i == id).map(|(_, s)| *s);
        assert_eq!(
            ranked.first().map(|(id, _)| *id),
            Some(10),
            "more theme ranks first"
        );
        assert_eq!(score(10), Some(1.0), "top thematic = 1.0");
        assert_eq!(score(30), None, "different creator + no theme is dropped");
        assert!(
            (score(20).unwrap() - (0.5 + CREATOR_BONUS)).abs() < 1e-4,
            "a liked creator adds a flat bonus, not a dominating magnitude term"
        );
    }
}
