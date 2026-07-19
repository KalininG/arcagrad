//! Filename-based series numbering and natural-order fallback.

use std::cmp::Ordering;
use std::sync::LazyLock;

use regex::Regex;

#[derive(Debug, Clone, PartialEq)]
pub struct LeafNumber {
    pub sort: f64,
    pub display: Option<String>,
    pub volume: Option<f64>,
    pub chapter: Option<f64>,
}

/// Volume markers, tried in order (first capture wins). A trailing range (`v16-17`)
/// captures only the first number: a leaf is one point on the axis, not a span.
static VOLUME_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)\bvol(?:ume)?\.?\s*(\d+(?:\.\d+)?)", // vol, vol. 10, Volume 01, Vol 7.5, Volume.2000
        r"(?i)\bv(\d+(?:\.\d+)?)",                 // v16, v01, v16-17 (first number)
        r"(?i)\btome\.?\s*(\d+(?:\.\d+)?)",        // tome 2, Tome.3 (French)
        r"(?i)\bt\.\s*(\d+(?:\.\d+)?)",            // t. 2 (dotted; bare `t3` is too false-prone)
        r"第(\d+(?:\.\d+)?)[卷册]",                // 第2卷 / 第2册 (Chinese)
        r"[卷册](\d+(?:\.\d+)?)",                  // 卷2 / 册2
        r"(\d+(?:\.\d+)?)巻",                      // 2巻 (Japanese)
    ]
    .iter()
    .map(|p| Regex::new(p).expect("static volume pattern compiles"))
    .collect()
});

static CHAPTER_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?i)\bch(?:apter)?\.?\s*(\d+(?:\.\d+)?)", // ch. 4, chapter 12, ch4
        r"\bc(\d+(?:\.\d+)?)",                      // c90, c90.5, c90-98 (lowercase c only)
        r"第(\d+(?:\.\d+)?)[话話]",                 // 第12话
        r"(\d+(?:\.\d+)?)[话話]",                   // 12话
    ]
    .iter()
    .map(|p| Regex::new(p).expect("static chapter pattern compiles"))
    .collect()
});

pub fn parse_leaf_number(stem: &str) -> Option<LeafNumber> {
    let volume = first_number(&VOLUME_PATTERNS, stem);
    let chapter = first_number(&CHAPTER_PATTERNS, stem);
    let sort = volume.or(chapter)?;
    let display = match (volume, chapter) {
        (Some(v), _) => Some(format!("Vol. {}", fmt_num(v))),
        (None, Some(c)) => Some(format!("Ch. {}", fmt_num(c))),
        _ => None,
    };
    Some(LeafNumber {
        sort,
        display,
        volume,
        chapter,
    })
}

pub fn chapter_marker(name: &str) -> Option<(f64, String)> {
    let n = first_number(&CHAPTER_PATTERNS, name)?;
    Some((n, format!("Ch. {}", fmt_num(n))))
}

fn first_number(patterns: &[Regex], s: &str) -> Option<f64> {
    for re in patterns {
        if let Some(n) = re
            .captures(s)
            .and_then(|c| c.get(1))
            .and_then(|m| m.as_str().parse::<f64>().ok())
        {
            return Some(n);
        }
    }
    None
}

fn fmt_num(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    match take_number(&mut ai).cmp(&take_number(&mut bi)) {
                        Ordering::Equal => continue,
                        ord => return ord,
                    }
                } else {
                    match ca.to_ascii_lowercase().cmp(&cb.to_ascii_lowercase()) {
                        Ordering::Equal => {
                            ai.next();
                            bi.next();
                        }
                        ord => return ord,
                    }
                }
            }
        }
    }
}

fn take_number(it: &mut std::iter::Peekable<std::str::Chars>) -> u128 {
    let mut n: u128 = 0;
    while let Some(d) = it.peek().and_then(|c| c.to_digit(10)) {
        n = n.saturating_mul(10).saturating_add(d as u128);
        it.next();
    }
    n
}

pub fn leaf_belongs_to_series(leaf_title: &str, series_name: &str) -> bool {
    let leaf = leaf_title.trim().to_lowercase();
    let series = series_name.trim().to_lowercase();
    if leaf.is_empty() || series.is_empty() {
        return false;
    }
    if leaf == series {
        return true;
    }
    leaf.strip_prefix(&series)
        .and_then(|rest| rest.chars().next())
        .is_some_and(|c| !c.is_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sort_of(stem: &str) -> Option<f64> {
        parse_leaf_number(stem).map(|n| n.sort)
    }

    #[test]
    fn leaf_belongs_to_series_is_word_boundary_prefix() {
        assert!(leaf_belongs_to_series(
            "Goodnight Punpun",
            "Goodnight Punpun"
        ));
        assert!(leaf_belongs_to_series(
            "Goodnight Punpun v03",
            "Goodnight Punpun"
        ));
        assert!(leaf_belongs_to_series(
            "Goodnight Punpun #2",
            "Goodnight Punpun"
        ));
        assert!(leaf_belongs_to_series(
            "berserk deluxe vol 1",
            "Berserk Deluxe"
        ));
        assert!(!leaf_belongs_to_series(
            "Kizumonogatari",
            "Monogatari Series"
        ));
        assert!(!leaf_belongs_to_series("Kizumonogatari", "Monogatari"));
        assert!(!leaf_belongs_to_series(
            "Bakemonogatari Part 01",
            "Monogatari"
        ));
        assert!(!leaf_belongs_to_series("Punpun", "Punp"));
        assert!(!leaf_belongs_to_series("", "Monogatari"));
        assert!(!leaf_belongs_to_series("Kizumonogatari", ""));
    }

    #[test]
    fn parses_v_forms_with_filename_metadata() {
        assert_eq!(
            sort_of("Atlas Chronicles v01 (2012) (Digital) (Library Copy)"),
            Some(1.0)
        );
        assert_eq!(
            sort_of("River Town v03 (2016) (Omnibus Edition) (Digital) (Library Copy)"),
            Some(3.0)
        );
        let n = parse_leaf_number("Atlas Chronicles v06 (2013) (Digital) (Library Copy)").unwrap();
        assert_eq!(n.volume, Some(6.0));
        assert_eq!(n.display.as_deref(), Some("Vol. 6"));
    }

    #[test]
    fn parses_vol_and_volume_forms() {
        assert_eq!(sort_of("Series vol. 10"), Some(10.0));
        assert_eq!(sort_of("Series Vol 7.5"), Some(7.5));
        assert_eq!(sort_of("Series Volume 01"), Some(1.0));
        assert_eq!(sort_of("Series Volume.2000"), Some(2000.0));
    }

    #[test]
    fn parses_chapter_forms_and_decimals() {
        assert_eq!(sort_of("Series c90.5"), Some(90.5));
        assert_eq!(sort_of("Series Chapter 12"), Some(12.0));
        assert_eq!(sort_of("Series ch. 4"), Some(4.0));
        let n = parse_leaf_number("Series c90.5").unwrap();
        assert_eq!(n.chapter, Some(90.5));
        assert_eq!(n.volume, None);
        assert_eq!(n.display.as_deref(), Some("Ch. 90.5"));
    }

    #[test]
    fn ranges_take_the_first_number() {
        assert_eq!(sort_of("Series v16-17"), Some(16.0));
        assert_eq!(sort_of("Series c90-98"), Some(90.0));
    }

    #[test]
    fn volume_wins_over_chapter_for_sort() {
        let n = parse_leaf_number("One Piece v02 c015").unwrap();
        assert_eq!(n.sort, 2.0, "volume is the sort scalar");
        assert_eq!(n.volume, Some(2.0));
        assert_eq!(n.chapter, Some(15.0), "chapter is still captured");
    }

    #[test]
    fn parses_cjk_forms() {
        assert_eq!(sort_of("作品 第2卷"), Some(2.0));
        assert_eq!(sort_of("作品 册2"), Some(2.0));
        assert_eq!(sort_of("作品 2巻"), Some(2.0));
        assert_eq!(sort_of("作品 第12话"), Some(12.0));
    }

    #[test]
    fn parses_tome_forms() {
        assert_eq!(sort_of("Série Tome 2"), Some(2.0));
        assert_eq!(sort_of("Série t. 3"), Some(3.0));
    }

    #[test]
    fn no_marker_returns_none() {
        assert_eq!(parse_leaf_number("[Artist] A Standalone Title"), None);
        assert_eq!(parse_leaf_number("Just A Title"), None);
    }

    #[test]
    fn uppercase_catalog_code_is_not_a_chapter() {
        assert_eq!(parse_leaf_number("(C91) [Studio] Some Title"), None);
        assert_eq!(sort_of("Some Series c91"), Some(91.0));
    }

    #[test]
    fn natural_cmp_orders_numbers_by_value() {
        assert_eq!(natural_cmp("v2", "v10"), Ordering::Less);
        assert_eq!(natural_cmp("Chapter 2", "Chapter 10"), Ordering::Less);
        assert_eq!(natural_cmp("v10", "v10"), Ordering::Equal);
        assert_eq!(natural_cmp("v02", "v3"), Ordering::Less);
        assert_eq!(natural_cmp("apple", "Banana"), Ordering::Less);
    }
}
