//! Environment-based server configuration.

use std::path::PathBuf;

use anyhow::{Context, Result};

pub struct Config {
    pub content_dir: PathBuf,
    pub data_dir: PathBuf,
    pub bind: String,
    /// Whether session cookies use the `Secure` flag.
    pub cookie_secure: bool,
    pub read_concurrency: usize,
    pub watch: bool,
    pub allow_private_repos: bool,
}

pub fn parse_bool(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// Cover-hash distance allowed when page counts differ.
pub const PHASH_MAX_HAMMING: u32 = 8;
/// Cover-hash distance allowed when page counts match.
pub const PHASH_SAME_PAGE_HAMMING: u32 = 12;

fn default_read_concurrency() -> usize {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2);
    heavy_read_concurrency(cores)
}

/// Reserve one core and cap concurrent image memory use.
fn heavy_read_concurrency(cores: usize) -> usize {
    cores.saturating_sub(1).clamp(1, 6)
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let content_dir: PathBuf = std::env::var("ARCA_CONTENT_DIR")
            .context("ARCA_CONTENT_DIR must be set (path to your archive library)")?
            .into();
        // Keep watcher paths consistent with scanner paths.
        let content_dir = std::fs::canonicalize(&content_dir).unwrap_or(content_dir);
        let data_dir = std::env::var("ARCA_DATA_DIR")
            .unwrap_or_else(|_| "./data".to_string())
            .into();
        let bind = std::env::var("ARCA_BIND").unwrap_or_else(|_| "0.0.0.0:3000".to_string());
        let cookie_secure = std::env::var("ARCA_COOKIE_SECURE")
            .ok()
            .and_then(|v| parse_bool(&v))
            .unwrap_or(false);
        let read_concurrency = std::env::var("ARCA_READ_CONCURRENCY")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&n| n >= 1)
            .unwrap_or_else(default_read_concurrency);
        let watch = std::env::var("ARCA_WATCH")
            .ok()
            .and_then(|v| parse_bool(&v))
            .unwrap_or(true);
        let allow_private_repos = std::env::var("ARCA_ALLOW_PRIVATE_REPOS")
            .ok()
            .and_then(|v| parse_bool(&v))
            .unwrap_or(false);

        Ok(Self {
            content_dir,
            data_dir,
            bind,
            cookie_secure,
            read_concurrency,
            watch,
            allow_private_repos,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{heavy_read_concurrency, parse_bool};

    #[test]
    fn parse_bool_recognizes_explicit_spellings_only() {
        for v in ["1", "true", "YES", " on "] {
            assert_eq!(parse_bool(v), Some(true), "{v}");
        }
        for v in ["0", "false", "No", "OFF"] {
            assert_eq!(parse_bool(v), Some(false), "{v}");
        }
        for v in ["", "banana", "2"] {
            assert_eq!(parse_bool(v), None, "{v}");
        }
    }

    #[test]
    fn reserves_a_core_floors_at_one_and_caps() {
        assert_eq!(heavy_read_concurrency(1), 1);
        assert_eq!(heavy_read_concurrency(2), 1);
        assert_eq!(heavy_read_concurrency(4), 3);
        assert_eq!(heavy_read_concurrency(8), 6);
        assert_eq!(heavy_read_concurrency(64), 6);
        assert_eq!(heavy_read_concurrency(0), 1);
    }
}
