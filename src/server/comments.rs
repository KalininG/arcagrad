//! Display filtering for scraped comments.

pub const REDACTED_LINK: &str = "[likely unsafe link removed]";

/// Replace anchors rejected by `keep`.
fn rewrite_anchors(body: &str, keep: impl Fn(&str, &str) -> bool) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(start) = next_anchor(rest) {
        out.push_str(&rest[..start]);
        let region = &rest[start..];
        match region.find("</a>") {
            Some(close) => {
                let anchor = &region[..close + "</a>".len()];
                let href = href_of(anchor).unwrap_or("");
                let text = strip_tags(between(anchor, ">", "</a>").unwrap_or(""));
                if keep(href, &text) {
                    out.push_str(anchor);
                } else {
                    out.push_str(REDACTED_LINK);
                }
                rest = &region[close + "</a>".len()..];
            }
            None => {
                out.push_str(REDACTED_LINK);
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

pub fn redact_links(body: &str) -> String {
    rewrite_anchors(body, |_href, _text| false)
}

pub fn sanitize_for_display(body: &str, downvoted: bool) -> String {
    let linked = if downvoted {
        redact_links(body)
    } else {
        redact_deceptive_links(body)
    };
    declutter(&linked)
}

const MAX_COMBINING_RUN: usize = 3;

fn declutter(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut run = 0usize;
    for c in body.chars() {
        if is_combining_mark(c) {
            run += 1;
            if run <= MAX_COMBINING_RUN {
                out.push(c);
            }
        } else {
            run = 0;
            out.push(c);
        }
    }
    out
}

fn is_combining_mark(c: char) -> bool {
    matches!(c as u32,
        0x0300..=0x036F   // Combining Diacritical Marks
        | 0x1AB0..=0x1AFF // …Extended
        | 0x1DC0..=0x1DFF // …Supplement
        | 0x20D0..=0x20FF // …for Symbols
        | 0xFE20..=0xFE2F // Combining Half Marks
    )
}

pub fn redact_deceptive_links(body: &str) -> String {
    rewrite_anchors(body, |href, text| match (host_of(href), host_of(text)) {
        (Some(h), Some(t)) => h == t,
        _ => true,
    })
}

fn next_anchor(s: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(rel) = s[from..].find("<a") {
        let at = from + rel;
        match s[at + 2..].chars().next() {
            Some(c) if c.is_whitespace() || c == '>' => return Some(at),
            _ => from = at + 2,
        }
    }
    None
}

fn between<'a>(s: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let i = s.find(start)? + start.len();
    let j = s[i..].find(end)? + i;
    Some(&s[i..j])
}

fn href_of(anchor: &str) -> Option<&str> {
    between(anchor, "href=\"", "\"").or_else(|| between(anchor, "href='", "'"))
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

fn host_of(s: &str) -> Option<String> {
    let s = s.trim();
    let s = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .unwrap_or(s);
    let host = s
        .split(['/', '?', '#', ' ', '\t', '\n'])
        .next()?
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    if !looks_like_host(host) {
        return None;
    }
    Some(host.to_string())
}

fn looks_like_host(h: &str) -> bool {
    if !h.contains('.') {
        return false;
    }
    let labels: Vec<&str> = h.split('.').collect();
    let tld = labels.last().copied().unwrap_or("");
    labels
        .iter()
        .all(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        && tld.len() >= 2
        && tld.chars().all(|c| c.is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_links_replaces_anchors_and_leaves_other_markup() {
        let body =
            "see <a href=\"http://spam.site\">this</a> and <a href='/g/2/'><strong>that</strong></a>!";
        let out = redact_links(body);
        assert!(!out.contains("<a"), "no anchor tags remain: {out}");
        assert!(!out.contains("spam.site"), "the url is gone: {out}");
        assert_eq!(out.matches(REDACTED_LINK).count(), 2, "both links redacted");
        assert!(
            out.starts_with("see ") && out.ends_with('!'),
            "surrounding text kept: {out}"
        );
        assert_eq!(
            redact_links("plain <strong>bold</strong> and an <article> tag"),
            "plain <strong>bold</strong> and an <article> tag"
        );
        assert_eq!(redact_links("just text"), "just text");
    }

    #[test]
    fn redact_deceptive_links_drops_mismatches_keeps_honest() {
        let d = redact_deceptive_links("<a href=\"http://evil.com/x\">openlibrary.org/g/123</a>");
        assert!(
            d.contains(REDACTED_LINK) && !d.contains("evil.com"),
            "deceptive link redacted: {d}"
        );

        let lang = redact_deceptive_links("<a href=\"https://anilist.co/g/2/abc/\">English</a>");
        assert!(
            lang.contains("anilist.co")
                && lang.contains("English")
                && !lang.contains(REDACTED_LINK),
            "language-name link kept: {lang}"
        );

        let rel = redact_deceptive_links("<a href=\"/g/2/abc/\">here</a>");
        assert!(rel.contains("/g/2/abc/") && !rel.contains(REDACTED_LINK));

        let same = redact_deceptive_links("<a href=\"https://anilist.co/g/2/\">anilist.co/g/2</a>");
        assert!(!same.contains(REDACTED_LINK), "matching host kept: {same}");

        let ver = redact_deceptive_links("<a href=\"http://evil.com\">chapter 1.5</a>");
        assert!(!ver.contains(REDACTED_LINK), "non-host text kept: {ver}");
    }

    #[test]
    fn declutter_caps_zalgo_but_keeps_real_accents() {
        let zalgo = format!("n{}Why?", "\u{0301}".repeat(50));
        let out = sanitize_for_display(&zalgo, false);
        let marks = out.chars().filter(|c| is_combining_mark(*c)).count();
        assert_eq!(marks, MAX_COMBINING_RUN, "the long combining run is capped");
        assert!(out.contains("Why?"), "the real text survives: {out:?}");
        assert!(out.starts_with('n'), "the base char survives: {out:?}");

        let accented = "cafe\u{0301} nin\u{0303}o";
        assert_eq!(sanitize_for_display(accented, false), accented);

        assert_eq!(sanitize_for_display("just text", false), "just text");
        let spam = format!("<a href=\"http://x.co\">y</a>{}", "\u{0301}".repeat(20));
        let out = sanitize_for_display(&spam, true);
        assert!(
            out.contains(REDACTED_LINK) && !out.contains("x.co"),
            "link still defanged: {out}"
        );
        assert!(
            out.chars().filter(|c| is_combining_mark(*c)).count() <= MAX_COMBINING_RUN,
            "trailing zalgo capped even after redaction: {out}"
        );
    }

    #[test]
    fn host_of_extracts_or_rejects() {
        assert_eq!(
            host_of("https://openlibrary.org/g/1/").as_deref(),
            Some("openlibrary.org")
        );
        assert_eq!(host_of("www.anilist.co").as_deref(), Some("anilist.co"));
        assert_eq!(host_of("here"), None);
        assert_eq!(host_of("/g/2/abc/"), None);
        assert_eq!(host_of("chapter 1.5"), None);
    }
}
