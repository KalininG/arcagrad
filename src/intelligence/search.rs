//! Tantivy index for ranked item search and title suggestions.
//!
//! The index is derived from SQLite and can be rebuilt at any time. Writes are queued by
//! `reindex_search`; request handlers only read it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use anyhow::{Context, Result};
use futures::TryStreamExt;
use sqlx::SqlitePool;
use tantivy::directory::MmapDirectory;
use tantivy::query::{BooleanQuery, Occur, Query, TermQuery};
use tantivy::schema::{
    Field, IndexRecordOption, Schema, Value, FAST, INDEXED, STORED, STRING, TEXT,
};
use tantivy::{
    doc, Index, IndexReader, IndexWriter, ReloadPolicy, Searcher, TantivyDocument, Term,
};

/// On-disk index format. A mismatch triggers a rebuild.
const SCHEMA_VERSION: &str = "2";

/// Tantivy requires at least 15 MB for the writer.
const WRITER_HEAP_BYTES: usize = 50_000_000;

// Shared projection for targeted and full indexing.
macro_rules! index_select {
    ($($tail:literal)?) => {
        concat!(
            "SELECT a.id, a.title, a.raw_title, a.kind, a.structural_hash, a.added_at, \
             COALESCE((SELECT group_concat(t.value, ' ') FROM item_tags it JOIN tags t ON t.id = it.tag_id \
             WHERE it.item_id = a.id), '') AS tags \
             FROM items a",
            $($tail)?
        )
    };
}
const SELECT_ONE: &str = index_select!(" WHERE a.id = ?");
const SELECT_ALL: &str = index_select!();

/// Allowed edit distance by token length.
fn fuzzy_distance(len: usize) -> Option<u8> {
    match len {
        0..=3 => None,
        4..=7 => Some(1),
        _ => Some(2),
    }
}

/// Maximum term expansions per token and field.
const PREFIX_EXPANSION_CAP: usize = 64;

/// Maximum terms inspected during fuzzy expansion.
const FUZZY_SCAN_CAP: usize = 4000;

/// Hits below this fraction of the best score are discarded.
const RELEVANCE_FLOOR_RATIO: f32 = 0.25;

/// Stopwords used only for ranked search queries.
const STOPWORDS: &[&str] = &[
    "a", "an", "the", "of", "and", "or", "in", "on", "to", "for", "is", "it", "no", "my", "at",
    "as", "by", "be", "with", "from", "wa", "ni", "wo", "ga", "mo", "de", "ka",
];

/// Restricted Damerau-Levenshtein comparison with adjacent transpositions.
fn within_edit_distance(a: &str, b: &str, max: usize) -> bool {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (la, lb) = (a.len(), b.len());
    if la.abs_diff(lb) > max {
        return false;
    }
    let mut d = vec![vec![0usize; lb + 1]; la + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=la {
        for j in 1..=lb {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut v = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                v = v.min(d[i - 2][j - 2] + 1);
            }
            d[i][j] = v;
        }
    }
    d[la][lb] <= max
}

/// Exclusive upper bound for a byte-ordered prefix range.
fn prefix_upper_bound(prefix: &str) -> Option<Vec<u8>> {
    let mut bytes = prefix.as_bytes().to_vec();
    while let Some(last) = bytes.last_mut() {
        if *last < 0xFF {
            *last += 1;
            return Some(bytes);
        }
        bytes.pop();
    }
    None
}

fn collapse_acronyms(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut run = String::new();
    let mut run_len = 0usize;
    let flush = |run: &mut String, run_len: &mut usize, out: &mut Vec<String>| {
        if *run_len >= 2 {
            out.push(std::mem::take(run));
        } else {
            run.clear();
        }
        *run_len = 0;
    };
    for tok in text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
    {
        if tok.chars().count() == 1 && tok.chars().all(|c| c.is_alphabetic()) {
            run.push_str(&tok.to_lowercase());
            run_len += 1;
        } else {
            flush(&mut run, &mut run_len, &mut out);
        }
    }
    flush(&mut run, &mut run_len, &mut out);
    out
}

struct Fields {
    id: Field,
    title: Field,
    title_romaji: Field,
    tags: Field,
    kind: Field,
    structural_hash: Field,
    added_at: Field,
}

/// Stored fields returned with a ranked search hit.
#[derive(Debug, Clone)]
pub struct TitleHit {
    pub id: i64,
    pub title: String,
    pub kind: String,
    pub structural_hash: String,
    pub score: f32,
}

/// SQLite projection used to build a Tantivy document.
#[derive(sqlx::FromRow)]
struct IndexRow {
    id: i64,
    title: String,
    raw_title: Option<String>,
    kind: String,
    structural_hash: String,
    added_at: i64,
    tags: String,
}

pub struct SearchIndex {
    /// Never held across an `.await`.
    writer: Mutex<IndexWriter>,
    reader: IndexReader,
    fields: Fields,
    dir: PathBuf,
    /// Set until a missing or stale index has been rebuilt.
    needs_rebuild: AtomicBool,
}

impl SearchIndex {
    fn build_schema() -> (Schema, Fields) {
        let mut b = Schema::builder();
        let id = b.add_u64_field("id", STORED | INDEXED);
        let title = b.add_text_field("title", TEXT | STORED);
        let title_romaji = b.add_text_field("title_romaji", TEXT);
        let tags = b.add_text_field("tags", TEXT);
        let kind = b.add_text_field("kind", STRING | STORED);
        let structural_hash = b.add_text_field("structural_hash", STRING | STORED);
        let added_at = b.add_i64_field("added_at", STORED | FAST);
        let schema = b.build();
        (
            schema,
            Fields {
                id,
                title,
                title_romaji,
                tags,
                kind,
                structural_hash,
                added_at,
            },
        )
    }

    /// Opens the index, recreating it when missing, stale, or unreadable.
    pub fn open_or_create(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("create search dir {}", dir.display()))?;
        let (schema, fields) = Self::build_schema();

        let version_ok = std::fs::read_to_string(dir.join("VERSION"))
            .map(|v| v.trim() == SCHEMA_VERSION)
            .unwrap_or(false);

        let mmap = MmapDirectory::open(&dir).context("open search MmapDirectory")?;
        let index = match Index::open_or_create(mmap, schema.clone()) {
            Ok(idx) if version_ok => idx,
            other => {
                if let Err(e) = other {
                    tracing::warn!("search index unreadable ({e:#}); rebuilding from scratch");
                } else {
                    tracing::info!("search schema v{SCHEMA_VERSION} mismatch; rebuilding index");
                }
                Self::wipe_dir(&dir)?;
                let mmap = MmapDirectory::open(&dir).context("reopen search MmapDirectory")?;
                Index::open_or_create(mmap, schema).context("recreate search index")?
            }
        };

        let writer = index
            .writer(WRITER_HEAP_BYTES)
            .context("create search IndexWriter")?;
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .context("create search IndexReader")?;

        Ok(Self {
            writer: Mutex::new(writer),
            reader,
            fields,
            dir,
            needs_rebuild: AtomicBool::new(!version_ok),
        })
    }

    /// Clears an index directory without removing the directory itself.
    fn wipe_dir(dir: &Path) -> Result<()> {
        for entry in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
            let path = entry?.path();
            if path.is_dir() {
                std::fs::remove_dir_all(&path).ok();
            } else {
                std::fs::remove_file(&path).ok();
            }
        }
        Ok(())
    }

    pub fn needs_rebuild(&self) -> bool {
        self.needs_rebuild.load(Ordering::Relaxed)
    }

    fn make_doc(&self, r: &IndexRow) -> TantivyDocument {
        let f = &self.fields;
        let mut romaji = r.raw_title.clone().unwrap_or_default();
        for acronym in collapse_acronyms(&r.title) {
            romaji.push(' ');
            romaji.push_str(&acronym);
        }
        doc!(
            f.id => r.id as u64,
            f.title => r.title.as_str(),
            f.title_romaji => romaji.as_str(),
            f.tags => r.tags.as_str(),
            f.kind => r.kind.as_str(),
            f.structural_hash => r.structural_hash.as_str(),
            f.added_at => r.added_at,
        )
    }

    /// Stages a full-document upsert, or a delete if the item is gone.
    pub async fn reindex_item(&self, pool: &SqlitePool, id: i64) -> Result<()> {
        let row: Option<IndexRow> = sqlx::query_as(SELECT_ONE)
            .bind(id)
            .fetch_optional(pool)
            .await
            .with_context(|| format!("load item {id} for search reindex"))?;
        let w = self.writer.lock().unwrap();
        w.delete_term(Term::from_field_u64(self.fields.id, id as u64));
        if let Some(r) = row {
            let doc = self.make_doc(&r);
            w.add_document(doc).context("stage search doc")?;
        }
        Ok(())
    }

    pub fn delete_item(&self, id: i64) {
        let w = self.writer.lock().unwrap();
        w.delete_term(Term::from_field_u64(self.fields.id, id as u64));
    }

    /// Commits staged writes and reloads the reader. This call blocks on fsync.
    pub fn commit(&self) -> Result<()> {
        {
            let mut w = self.writer.lock().unwrap();
            w.commit().context("commit search index")?;
        }
        self.reader.reload().context("reload search reader")?;
        Ok(())
    }

    /// Rebuilds the full index from SQLite and updates its version marker.
    pub async fn rebuild_from_db(&self, pool: &SqlitePool) -> Result<usize> {
        {
            let w = self.writer.lock().unwrap();
            w.delete_all_documents().context("clear search index")?;
        }
        let mut stream = sqlx::query_as::<_, IndexRow>(SELECT_ALL).fetch(pool);
        let mut n = 0usize;
        while let Some(r) = stream
            .try_next()
            .await
            .context("stream items for rebuild")?
        {
            let doc = self.make_doc(&r);
            {
                let w = self.writer.lock().unwrap();
                w.add_document(doc).context("stage search doc (rebuild)")?;
            }
            n += 1;
        }
        {
            let mut w = self.writer.lock().unwrap();
            w.commit().context("commit search rebuild")?;
        }
        self.reader.reload().context("reload search reader")?;
        std::fs::write(self.dir.join("VERSION"), SCHEMA_VERSION).context("write search VERSION")?;
        self.needs_rebuild.store(false, Ordering::Relaxed);
        Ok(n)
    }

    /// Ranked title suggestions using the same matcher as full search.
    pub fn suggest_titles(&self, prefix: &str, limit: usize) -> Result<Vec<TitleHit>> {
        self.ranked_hits(prefix, limit)
    }

    /// Expands a prefix to indexed terms so BM25 can score each term normally.
    fn expand_prefix(&self, searcher: &Searcher, field: Field, prefix: &str) -> Vec<String> {
        let mut out: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let upper = prefix_upper_bound(prefix);
        for seg in searcher.segment_readers() {
            let Ok(inv) = seg.inverted_index(field) else {
                continue;
            };
            let builder = inv.terms().range().ge(prefix.as_bytes());
            let builder = match &upper {
                Some(u) => builder.lt(u.as_slice()),
                None => builder,
            };
            let Ok(mut stream) = builder.into_stream() else {
                continue;
            };
            while stream.advance() {
                if let Ok(t) = std::str::from_utf8(stream.key()) {
                    out.insert(t.to_string());
                    if out.len() >= PREFIX_EXPANSION_CAP {
                        break;
                    }
                }
            }
            if out.len() >= PREFIX_EXPANSION_CAP {
                break;
            }
        }
        out.into_iter().collect()
    }

    /// Expands a token to same-initial terms within the requested edit distance.
    fn expand_fuzzy(
        &self,
        searcher: &Searcher,
        field: Field,
        token: &str,
        dist: usize,
    ) -> Vec<String> {
        let Some(first) = token.chars().next() else {
            return Vec::new();
        };
        let lead = first.to_string();
        let upper = prefix_upper_bound(&lead);
        let mut out: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut scanned = 0usize;
        for seg in searcher.segment_readers() {
            let Ok(inv) = seg.inverted_index(field) else {
                continue;
            };
            let builder = inv.terms().range().ge(lead.as_bytes());
            let builder = match &upper {
                Some(u) => builder.lt(u.as_slice()),
                None => builder,
            };
            let Ok(mut stream) = builder.into_stream() else {
                continue;
            };
            while stream.advance() {
                scanned += 1;
                if scanned > FUZZY_SCAN_CAP || out.len() >= PREFIX_EXPANSION_CAP {
                    break;
                }
                if let Ok(t) = std::str::from_utf8(stream.key()) {
                    if within_edit_distance(t, token, dist) {
                        out.insert(t.to_string());
                    }
                }
            }
            if out.len() >= PREFIX_EXPANSION_CAP {
                break;
            }
        }
        out.into_iter().collect()
    }

    /// Ranked full-text search over titles, romaji, and tag values.
    pub fn search_ids(&self, query_text: &str, limit: usize) -> Result<Vec<(i64, f32)>> {
        Ok(self
            .ranked_hits(query_text, limit)?
            .into_iter()
            .map(|h| (h.id, h.score))
            .collect())
    }

    /// Shared matcher for search results and autocomplete.
    fn ranked_hits(&self, query_text: &str, limit: usize) -> Result<Vec<TitleHit>> {
        let active: Vec<String> = query_text
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
            .map(|t| t.to_lowercase())
            .filter(|t| !(t.chars().count() == 1 && t.chars().all(|c| c.is_alphabetic())))
            .filter(|t| !STOPWORDS.contains(&t.as_str()))
            .collect();
        if active.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let last = active.len() - 1;
        let f = &self.fields;
        let content = [f.title, f.title_romaji, f.tags];
        let searcher = self.reader.searcher();
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        // Title matches are retained for coordination scoring below.
        let mut token_terms: Vec<std::collections::HashSet<String>> =
            vec![std::collections::HashSet::new(); active.len()];
        let push = |field: Field, term_str: &str, clauses: &mut Vec<(Occur, Box<dyn Query>)>| {
            let q = TermQuery::new(
                Term::from_field_text(field, term_str),
                IndexRecordOption::WithFreqs,
            );
            clauses.push((Occur::Should, Box::new(q) as Box<dyn Query>));
        };
        for (i, tok) in active.iter().enumerate() {
            let len = tok.chars().count();
            for field in content {
                let terms = if i == last {
                    self.expand_prefix(&searcher, field, tok)
                } else {
                    vec![tok.clone()]
                };
                let fuzzy = fuzzy_distance(len)
                    .map(|d| self.expand_fuzzy(&searcher, field, tok, d as usize))
                    .unwrap_or_default();
                for t in terms.iter().chain(&fuzzy) {
                    push(field, t, &mut clauses);
                }
                if field == f.title {
                    token_terms[i].extend(terms);
                    token_terms[i].extend(fuzzy);
                }
            }
        }
        if clauses.is_empty() {
            return Ok(Vec::new());
        }
        let query = BooleanQuery::new(clauses);
        let top = searcher
            .search(
                &query,
                &tantivy::collector::TopDocs::with_limit(limit).order_by_score(),
            )
            .context("relevance search")?;

        // Favor titles that match more distinct query tokens.
        let mut ranked: Vec<TitleHit> = top
            .into_iter()
            .filter_map(|(score, addr)| {
                let d: TantivyDocument = searcher.doc(addr).ok()?;
                let get_str = |f: Field| {
                    d.get_first(f)
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string()
                };
                let id = d.get_first(self.fields.id).and_then(|v| v.as_u64())? as i64;
                let title = get_str(self.fields.title);
                let mut words: std::collections::HashSet<String> = title
                    .split(|c: char| !c.is_alphanumeric())
                    .filter(|w| !w.is_empty())
                    .map(|w| w.to_lowercase())
                    .collect();
                words.extend(collapse_acronyms(&title));
                let matched = token_terms
                    .iter()
                    .filter(|ts| ts.iter().any(|t| words.contains(t)))
                    .count()
                    .max(1);
                Some(TitleHit {
                    id,
                    kind: get_str(self.fields.kind),
                    structural_hash: get_str(self.fields.structural_hash),
                    title,
                    score: score * matched as f32,
                })
            })
            .collect();
        ranked.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let floor = ranked
            .first()
            .map_or(0.0, |h| h.score * RELEVANCE_FLOOR_RATIO);
        ranked.retain(|h| h.score >= floor);
        Ok(ranked)
    }

    /// Number of committed documents in the current reader snapshot.
    pub fn doc_count(&self) -> u64 {
        self.reader.searcher().num_docs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_index() -> (tempfile::TempDir, SearchIndex) {
        let dir = tempfile::tempdir().unwrap();
        let idx = SearchIndex::open_or_create(dir.path()).unwrap();
        (dir, idx)
    }

    fn add(idx: &SearchIndex, id: i64, title: &str, raw: &str, tags: &str) {
        let r = IndexRow {
            id,
            title: title.to_string(),
            raw_title: Some(raw.to_string()),
            kind: "comics".into(),
            structural_hash: format!("hash{id}"),
            added_at: id,
            tags: tags.to_string(),
        };
        let w = idx.writer.lock().unwrap();
        w.add_document(idx.make_doc(&r)).unwrap();
    }

    #[test]
    fn prefix_matches_partial_trailing_token() {
        let (_d, idx) = temp_index();
        add(&idx, 1, "Of Mice and Men", "of mice and men", "");
        add(&idx, 2, "To the Lighthouse", "To the Lighthouses", "");
        idx.commit().unwrap();

        let hits = idx.suggest_titles("of mic", 10).unwrap();
        let ids: Vec<i64> = hits.iter().map(|h| h.id).collect();
        assert!(ids.contains(&1), "of mic should match 'Of Mice and Men'");
        let men = hits.iter().find(|h| h.id == 1).unwrap();
        assert_eq!(men.title, "Of Mice and Men");
        assert_eq!(men.structural_hash, "hash1");
    }

    #[test]
    fn single_token_prefix_and_relevance_order() {
        let (_d, idx) = temp_index();
        add(&idx, 1, "Mice", "mice", "");
        add(&idx, 2, "Microscope Manual", "microscope manual", "");
        idx.commit().unwrap();
        let hits = idx.suggest_titles("mi", 10).unwrap();
        assert_eq!(hits.len(), 2, "both titles start with 'mi'");
    }

    #[test]
    fn tag_words_match_both_surfaces() {
        let (_d, idx) = temp_index();
        add(&idx, 1, "Untitled Work", "untitled work", "mystery drama");
        idx.commit().unwrap();
        let hits = idx.search_ids("mystery", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, 1);
        let sug = idx.suggest_titles("mystery", 10).unwrap();
        assert_eq!(sug.len(), 1);
        assert_eq!(sug[0].id, 1);
    }

    #[test]
    fn relevance_tolerates_typos() {
        let (_d, idx) = temp_index();
        add(&idx, 1, "Microscope Manual", "microscope manual", "");
        idx.commit().unwrap();
        let hits = idx.search_ids("microscpe", 10).unwrap();
        assert_eq!(hits.len(), 1, "typo'd query still matches");
        assert_eq!(hits[0].0, 1);
        assert!(idx.search_ids("xyz", 10).unwrap().is_empty());
    }

    #[test]
    fn relevance_matches_word_prefix() {
        let (_d, idx) = temp_index();
        add(&idx, 1, "Starlight Library Adventures", "starlight", "");
        add(&idx, 2, "Something Else", "something else", "");
        idx.commit().unwrap();
        let hits = idx.search_ids("star", 10).unwrap();
        assert!(
            hits.iter().any(|h| h.0 == 1),
            "prefix 'star' matches Starlight"
        );
        assert!(!hits.iter().any(|h| h.0 == 2));
        assert!(idx.search_ids("st", 10).unwrap().iter().any(|h| h.0 == 1));
    }

    #[test]
    fn exact_outranks_fuzzy() {
        let (_d, idx) = temp_index();
        add(&idx, 1, "Romance", "romance", "");
        add(&idx, 2, "Romanae", "romanae", "");
        idx.commit().unwrap();
        let hits = idx.search_ids("romance", 10).unwrap();
        assert_eq!(hits[0].0, 1, "exact match ranks above a fuzzy one");
    }

    #[test]
    fn rare_prefix_word_outranks_common_exact_word() {
        let (_d, idx) = temp_index();
        add(&idx, 1, "Moonflower Archive", "moonflower archive", "");
        add(&idx, 2, "North Moon Chronicle", "north moon chronicle", "");
        add(&idx, 3, "Moon Map Stories", "moon map stories", "");
        add(&idx, 4, "Under the Moon", "under the moon", "");
        idx.commit().unwrap();
        let hits = idx.search_ids("moon", 10).unwrap();
        assert_eq!(hits[0].0, 1, "rare prefix word outranks common exact word");
    }

    #[test]
    fn collapse_acronyms_joins_single_letter_runs() {
        assert_eq!(collapse_acronyms("S.T.A.R"), vec!["star"]);
        assert_eq!(collapse_acronyms("C.O.M.E.T"), vec!["comet"]);
        assert_eq!(
            collapse_acronyms("A Flight of Swallows"),
            Vec::<String>::new()
        );
        assert_eq!(collapse_acronyms("OFFICE HOURS"), Vec::<String>::new());
    }

    #[test]
    fn dotted_acronym_matches_collapsed_query() {
        let (_d, idx) = temp_index();
        add(&idx, 1, "S.T.A.R", "s.t.a.r", "");
        add(&idx, 2, "Other Title", "other title", "");
        idx.commit().unwrap();
        let hits = idx.search_ids("star", 10).unwrap();
        assert!(hits.iter().any(|h| h.0 == 1), "'star' finds S.T.A.R");
        assert!(!hits.iter().any(|h| h.0 == 2));
    }

    #[test]
    fn common_middle_word_stays_exact_not_prefix() {
        let (_d, idx) = temp_index();
        add(&idx, 1, "A Flight of Swallows", "a flight of swallows", "");
        add(&idx, 2, "Office Hours", "office hours", "");
        idx.commit().unwrap();
        let hits = idx.search_ids("fligth of swallows", 10).unwrap();
        assert!(hits.iter().any(|h| h.0 == 1));
        assert!(
            !hits.iter().any(|h| h.0 == 2),
            "a common middle word doesn't prefix-expand to unrelated titles"
        );
    }

    #[test]
    fn matching_more_content_words_wins() {
        let (_d, idx) = temp_index();
        add(&idx, 1, "A Flight of Swallows", "a flight of swallows", "");
        add(&idx, 2, "River Swing", "river swing", "");
        add(&idx, 3, "Summer Swan", "summer swan", "");
        add(&idx, 4, "Winter Swell", "winter swell", "");
        idx.commit().unwrap();
        let hits = idx.search_ids("a fligt of sw", 10).unwrap();
        assert_eq!(
            hits[0].0, 1,
            "two content-word match ranks first, got {hits:?}"
        );
    }

    #[test]
    fn edit_distance_counts_transposition_as_one() {
        assert!(within_edit_distance("fligth", "flight", 1));
        assert!(within_edit_distance("fligt", "flight", 1));
        assert!(!within_edit_distance("flint", "flight", 1));
    }

    #[test]
    fn stopwords_dont_pull_unrelated_titles() {
        let (_d, idx) = temp_index();
        add(&idx, 1, "A Flight of Swallows", "a flight of swallows", "");
        add(&idx, 2, "River of Lanterns", "river of lanterns", "");
        add(
            &idx,
            3,
            "Clouds of Morning Rain",
            "clouds of morning rain",
            "",
        );
        idx.commit().unwrap();
        let hits = idx.search_ids("a fligth of swallows", 10).unwrap();
        assert_eq!(
            hits.len(),
            1,
            "only the strong match survives, got {hits:?}"
        );
        assert_eq!(hits[0].0, 1);
    }

    #[test]
    fn single_char_tokens_are_ignored() {
        let (_d, idx) = temp_index();
        add(&idx, 1, "S T A R", "s t a r", "");
        add(&idx, 2, "Swallow Stories", "swallow stories", "");
        idx.commit().unwrap();
        let hits = idx.search_ids("a swallow", 10).unwrap();
        assert!(hits.iter().any(|h| h.0 == 2));
        assert!(
            !hits.iter().any(|h| h.0 == 1),
            "a 1-char token is noise, so the abbreviation title doesn't match"
        );
    }

    #[test]
    fn fuzzy_distance_is_length_gated() {
        assert_eq!(fuzzy_distance(3), None);
        assert_eq!(fuzzy_distance(5), Some(1));
        assert_eq!(fuzzy_distance(9), Some(2));
    }

    #[test]
    fn delete_removes_from_results() {
        let (_d, idx) = temp_index();
        add(&idx, 1, "Deletable", "deletable", "");
        idx.commit().unwrap();
        assert_eq!(idx.suggest_titles("delet", 10).unwrap().len(), 1);
        idx.delete_item(1);
        idx.commit().unwrap();
        assert_eq!(idx.suggest_titles("delet", 10).unwrap().len(), 0);
    }

    async fn db_insert(pool: &SqlitePool, title: &str, raw: &str, hash: &str) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO items \
             (scheme_tag, structural_hash, path, size_bytes, mtime, format, title, raw_title, kind, modality, added_at, last_modified_at) \
             VALUES ('zip-structural-v1', ?, ?, 1, 1, 'cbz', ?, ?, 'comics', 'paginated', 1, 0) RETURNING id",
        )
        .bind(hash)
        .bind(format!("/p/{hash}"))
        .bind(title)
        .bind(raw)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[sqlx::test]
    async fn reindex_and_rebuild_from_db(pool: SqlitePool) {
        let id1 = db_insert(&pool, "Of Mice and Men", "of mice and men", "hashA").await;
        let id2 = db_insert(&pool, "Office Hours", "office hours", "hashB").await;
        let dir = tempfile::tempdir().unwrap();
        let idx = SearchIndex::open_or_create(dir.path()).unwrap();
        assert!(idx.needs_rebuild(), "fresh dir needs a rebuild");

        idx.reindex_item(&pool, id1).await.unwrap();
        idx.reindex_item(&pool, id2).await.unwrap();
        idx.commit().unwrap();
        let men = idx
            .suggest_titles("of mic", 10)
            .unwrap()
            .into_iter()
            .find(|h| h.id == id1)
            .expect("of mic matches Of Mice and Men");
        assert_eq!(men.title, "Of Mice and Men");
        assert_eq!(men.structural_hash, "hashA");

        idx.rebuild_from_db(&pool).await.unwrap();
        assert_eq!(idx.doc_count(), 2);
        assert!(!idx.needs_rebuild(), "rebuild clears the flag");
        assert!(idx
            .suggest_titles("office", 10)
            .unwrap()
            .iter()
            .any(|h| h.id == id2));

        sqlx::query("DELETE FROM items WHERE id = ?")
            .bind(id2)
            .execute(&pool)
            .await
            .unwrap();
        idx.reindex_item(&pool, id2).await.unwrap();
        idx.commit().unwrap();
        assert_eq!(idx.doc_count(), 1);
        assert!(idx.suggest_titles("office", 10).unwrap().is_empty());
    }
}
