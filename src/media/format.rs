//! Canonical registry of supported container formats and upload policy.

use std::path::Path;

pub struct Format {
    /// Lowercase file extension, no dot.
    pub ext: &'static str,
    pub uploadable: bool,
}

/// Every format the scanner/watcher will index. Add a format here and nowhere else.
pub const FORMATS: &[Format] = &[
    Format {
        ext: "cbz",
        uploadable: true,
    },
    Format {
        ext: "zip",
        uploadable: true,
    },
    Format {
        ext: "cbr",
        uploadable: true,
    },
    Format {
        ext: "rar",
        uploadable: true,
    },
    Format {
        ext: "epub",
        uploadable: true,
    },
];

pub fn ext_of(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
}

pub fn is_supported(path: &Path) -> bool {
    ext_of(path).is_some_and(|e| FORMATS.iter().any(|f| f.ext == e))
}

pub fn is_uploadable(path: &Path) -> bool {
    ext_of(path).is_some_and(|e| FORMATS.iter().any(|f| f.ext == e && f.uploadable))
}

pub fn uploadable_exts() -> Vec<&'static str> {
    FORMATS
        .iter()
        .filter(|f| f.uploadable)
        .map(|f| f.ext)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn supported_covers_every_format_uploadable_is_a_subset() {
        for f in FORMATS {
            let p = format!("book.{}", f.ext);
            assert!(is_supported(Path::new(&p)), "{} must be supported", f.ext);
            assert_eq!(
                is_uploadable(Path::new(&p)),
                f.uploadable,
                "{} uploadable flag",
                f.ext
            );
        }
        assert!(is_supported(Path::new("x.epub")));
        assert!(is_uploadable(Path::new("x.epub")));
        assert!(is_supported(Path::new("X.CBZ")));
        assert!(!is_supported(Path::new("notes.txt")));
        assert!(!is_supported(Path::new("noext")));
    }
}
