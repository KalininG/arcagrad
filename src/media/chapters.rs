//! In-archive chapter detection over an item's ordered page stream.

#[derive(Debug, Clone, PartialEq)]
pub struct Chapter {
    pub number_sort: f64,
    /// Display label (`"Ch. 1"`, `"Ch. 12.5"`), or `None` for the front-matter run.
    pub number_disp: Option<String>,
    /// A readable title when known: only the front-matter run carries one (`"Front matter"`);
    /// in-archive page names don't encode chapter titles (a future ComicInfo ingest would).
    pub title: Option<String>,
    /// 0-based index of this chapter's first page in the item's whole page stream.
    pub start_page: usize,
    /// How many pages this chapter spans.
    pub page_count: usize,
}

const MIN_CHAPTERS: usize = 2;

const FRONT_MATTER: &str = "Front matter";

pub fn parse_chapters(pages: &[String]) -> Vec<Chapter> {
    struct Group {
        num: Option<f64>,
        disp: Option<String>,
        start: usize,
        count: usize,
    }
    let mut groups: Vec<Group> = Vec::new();

    for (i, name) in pages.iter().enumerate() {
        match crate::media::series::chapter_marker(name) {
            Some((n, disp)) => match groups.last_mut() {
                Some(g) if g.num == Some(n) => g.count += 1,
                _ => groups.push(Group {
                    num: Some(n),
                    disp: Some(disp),
                    start: i,
                    count: 1,
                }),
            },
            None => match groups.last_mut() {
                Some(g) => g.count += 1,
                None => groups.push(Group {
                    num: None,
                    disp: None,
                    start: i,
                    count: 1,
                }),
            },
        }
    }

    if groups.iter().filter(|g| g.num.is_some()).count() < MIN_CHAPTERS {
        return Vec::new();
    }

    groups
        .into_iter()
        .map(|g| Chapter {
            number_sort: g.num.unwrap_or(0.0),
            number_disp: g.disp,
            title: g.num.is_none().then(|| FRONT_MATTER.to_string()),
            start_page: g.start,
            page_count: g.count,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn webtoon_ch_prefix_with_preamble() {
        let mut pages = names(&[
            "0001_0000.webp",
            "0002_0000.webp",
            "0003_0000_1.webp",
            "0004_0000_2.jpg",
            "0005_0000_3.webp",
            "0006_0000_4.jpg",
            "0007_0000_5.jpg",
        ]);
        for ch in 1..=3 {
            for p in 1..=4 {
                pages.push(format!("CH{ch:03}_{p:03}.jpg"));
            }
        }
        let chapters = parse_chapters(&pages);
        assert_eq!(chapters.len(), 4);
        assert_eq!(chapters[0].number_disp, None);
        assert_eq!(chapters[0].title.as_deref(), Some("Front matter"));
        assert_eq!(chapters[0].start_page, 0);
        assert_eq!(chapters[0].page_count, 7);
        assert_eq!(chapters[1].number_disp.as_deref(), Some("Ch. 1"));
        assert_eq!(chapters[1].number_sort, 1.0);
        assert_eq!(chapters[1].start_page, 7);
        assert_eq!(chapters[1].page_count, 4);
        assert_eq!(chapters[3].number_disp.as_deref(), Some("Ch. 3"));
        assert_eq!(chapters[3].start_page, 15);
    }

    #[test]
    fn omnibus_lowercase_c_embedded() {
        let mut pages = Vec::new();
        for ch in 1..=3 {
            for p in 0..5 {
                pages.push(format!(
                    "Goodnight Punpun - c{ch:03} (v01) - p{p:03} [VIZ Media] [Digital] [1r0n].png"
                ));
            }
        }
        let chapters = parse_chapters(&pages);
        assert_eq!(
            chapters.len(),
            3,
            "no front matter, so exactly the 3 chapters"
        );
        assert_eq!(chapters[0].number_disp.as_deref(), Some("Ch. 1"));
        assert_eq!(chapters[0].start_page, 0);
        assert_eq!(chapters[0].page_count, 5);
        assert_eq!(chapters[2].number_disp.as_deref(), Some("Ch. 3"));
        assert_eq!(chapters[2].start_page, 10);
    }

    #[test]
    fn flat_gallery_is_not_chaptered() {
        let pages = names(&["001.jpg", "002.jpg", "003.jpg", "004.jpg", "005.jpg"]);
        assert!(parse_chapters(&pages).is_empty());
    }

    #[test]
    fn single_chapter_is_not_chaptered() {
        let pages = names(&["c001_001.jpg", "c001_002.jpg", "c001_003.jpg"]);
        assert!(parse_chapters(&pages).is_empty());
    }

    #[test]
    fn decimal_chapter_is_its_own_group() {
        let pages = names(&[
            "c012_001.jpg",
            "c012_002.jpg",
            "c012.5_001.jpg",
            "c013_001.jpg",
            "c013_002.jpg",
        ]);
        let chapters = parse_chapters(&pages);
        let disps: Vec<_> = chapters
            .iter()
            .map(|c| c.number_disp.as_deref().unwrap())
            .collect();
        assert_eq!(disps, ["Ch. 12", "Ch. 12.5", "Ch. 13"]);
        assert_eq!(chapters[1].number_sort, 12.5);
    }

    #[test]
    fn trailing_unnumbered_pages_extend_the_last_chapter() {
        let pages = names(&[
            "c001_001.jpg",
            "c001_002.jpg",
            "c002_001.jpg",
            "c002_002.jpg",
            "zzz_credits.jpg",
        ]);
        let chapters = parse_chapters(&pages);
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[1].page_count, 3, "credits page folds into ch.2");
    }
}
