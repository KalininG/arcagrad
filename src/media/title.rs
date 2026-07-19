//! Display-title cleanup for archive filenames.

/// Extract a display title from a filename stem.
pub fn clean(raw: &str) -> String {
    let mut stripped = String::with_capacity(raw.len());
    let mut depth = 0i32;
    for c in raw.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = (depth - 1).max(0),
            _ if depth == 0 => stripped.push(c),
            _ => {}
        }
    }

    let title = stripped
        .split(['|', '｜', '︱'])
        .flat_map(|part| part.split(" / "))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .enumerate()
        .min_by_key(|(i, s)| {
            (
                s.chars().filter(|c| !c.is_ascii()).count(),
                std::cmp::Reverse(*i),
            )
        })
        .map(|(_, s)| strip_series_suffix(&collapse_ws(s)))
        .unwrap_or_default();

    if title.is_empty() {
        collapse_ws(raw)
    } else {
        title
    }
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_series_suffix(title: &str) -> String {
    const KW: &[&str] = &[
        "chapter", "chapters", "ch", "vol", "vols", "volume", "volumes", "episode", "episodes",
    ];
    let words: Vec<&str> = title.split_whitespace().collect();
    for i in 0..words.len() {
        let kw = words[i].trim_end_matches('.').to_ascii_lowercase();
        let next_is_num = words
            .get(i + 1)
            .and_then(|n| n.chars().next())
            .is_some_and(|c| c.is_ascii_digit());
        let is_marker = (KW.contains(&kw.as_str()) && next_is_num) || fused_series_marker(words[i]);
        if is_marker {
            let kept = words[..i].join(" ");
            return if kept.is_empty() {
                title.to_string()
            } else {
                kept
            };
        }
    }
    title.to_string()
}

fn fused_series_marker(word: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "volume", "volumes", "vol", "vols", "v", "chapter", "chapters", "ch", "episode",
        "episodes", "ep",
    ];
    let lower = word.to_ascii_lowercase();
    PREFIXES.iter().any(|p| {
        lower
            .strip_prefix(p)
            .map(|rest| rest.strip_prefix('.').unwrap_or(rest))
            .is_some_and(|rest| rest.chars().next().is_some_and(|c| c.is_ascii_digit()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_brackets_and_prefers_english_side() {
        let raw = "[Northwind Press (A. Reed)] Hoshi no Tabi ︱ Journey Among the Stars [English] [Proofread] [Digital] {revised}";
        assert_eq!(clean(raw), "Journey Among the Stars");
    }

    #[test]
    fn leading_event_and_circle_are_dropped() {
        assert_eq!(
            clean("(Expo 90) [Studio (Creator)] Just The Title [English]"),
            "Just The Title"
        );
    }

    #[test]
    fn bare_slash_is_kept_but_spaced_slash_splits() {
        assert_eq!(clean("Fate/Grand Order"), "Fate/Grand Order");
        assert_eq!(
            clean("Example popular/week/fantasy/2"),
            "Example popular/week/fantasy/2"
        );
        assert_eq!(clean("かなタイトル / English Title"), "English Title");
    }

    #[test]
    fn kana_side_is_rejected_in_favor_of_english() {
        assert_eq!(clean("[Artist] タイトル | English Title"), "English Title");
    }

    #[test]
    fn english_first_pair_still_picks_english() {
        assert_eq!(clean("English Title | かなタイトル"), "English Title");
    }

    #[test]
    fn a_plain_name_is_taken_whole() {
        assert_eq!(clean("My Comic"), "My Comic");
        assert_eq!(clean("book-0"), "book-0");
    }

    #[test]
    fn nested_and_interleaved_brackets() {
        assert_eq!(clean("[A (B)] Core Title (Parody) {v2}"), "Core Title");
    }

    #[test]
    fn all_brackets_falls_back_to_raw() {
        assert_eq!(clean("[only] [brackets]"), "[only] [brackets]");
    }

    #[test]
    fn collapses_whitespace() {
        assert_eq!(clean("[x]   Spaced    Out   Title  "), "Spaced Out Title");
    }

    #[test]
    fn no_english_side_keeps_romaji() {
        assert_eq!(clean("[Author] Hoshi no Tabi"), "Hoshi no Tabi");
    }

    #[test]
    fn trims_trailing_chapter_range_and_status() {
        assert_eq!(
            clean("Example Webtoon Chapter 1 - 100 Completed"),
            "Example Webtoon"
        );
        assert_eq!(clean("My Hero Academia Chapter 1-362"), "My Hero Academia");
        assert_eq!(clean("One Piece Vol. 1-100 Ongoing"), "One Piece");
        assert_eq!(clean("Some Title Episode 12"), "Some Title");
    }

    #[test]
    fn trims_fused_volume_chapter_markers() {
        assert_eq!(clean("Vagabond v01-37"), "Vagabond");
        assert_eq!(clean("Vagabond v01"), "Vagabond");
        assert_eq!(clean("Attack on Titan vol.05"), "Attack on Titan");
        assert_eq!(clean("One Piece ch1012"), "One Piece");
        assert_eq!(clean("Berserk ch1-50 Ongoing"), "Berserk");
    }

    #[test]
    fn fused_marker_false_positive_guards() {
        assert_eq!(clean("Vagabond"), "Vagabond");
        assert_eq!(clean("Chainsaw Man"), "Chainsaw Man");
        assert_eq!(
            clean("C3 Cube x Cursed x Curious"),
            "C3 Cube x Cursed x Curious"
        );
    }

    #[test]
    fn chapter_word_without_a_number_is_kept() {
        assert_eq!(clean("The Final Chapter"), "The Final Chapter");
        assert_eq!(clean("Chapter 1"), "Chapter 1");
    }
}
