//! Article cache — stores fetched articles in `~/.odoru/articles/`.
//!
//! Each cached article lives in its own directory named after the URL:
//!   `~/.odoru/articles/<hostname>-<slugified-path>/`
//!     - `article.md`  — YAML frontmatter (metadata) + markdown body
//!     - `article.txt` — plain text (no frontmatter)
//!
//! Lookup is by URL: scan directories, match the `url` frontmatter field.
//! This is fast enough for a personal collection.

use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::frontmatter;

// ---------------------------------------------------------------------------
// Frontmatter schema for cached article.md
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct ArticleFrontmatter {
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default)]
    authors: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    cached_at: String,
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A cached article, ready to use without re-fetching.
#[derive(Debug, Clone)]
pub struct CachedArticle {
    pub url: String,
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub date: Option<String>,
    pub description: Option<String>,
    /// Markdown content.
    pub content: String,
    /// Plain text content (for TTS).
    pub plain_text: String,
}

// ---------------------------------------------------------------------------
// Cache directory
// ---------------------------------------------------------------------------

/// Returns `~/.odoru/articles/`, creating it if needed.
pub fn cache_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME")
        .context("$HOME not set")?;
    let dir = PathBuf::from(home).join(".odoru").join("articles");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create cache dir {}", dir.display()))?;
    Ok(dir)
}

// ---------------------------------------------------------------------------
// URL → directory name
// ---------------------------------------------------------------------------

/// Derive a cache directory name from a URL.
///
/// `https://ultrasaurus.com/2015/10/software-isnt-real/`
///   → `ultrasaurus-com-2015-10-software-isnt-real`
pub fn url_to_slug(url: &str) -> String {
    let parsed = url::Url::parse(url).ok();

    let host = parsed
        .as_ref()
        .and_then(|u| u.host_str())
        .unwrap_or("unknown");

    let path = parsed
        .as_ref()
        .map(|u| u.path())
        .unwrap_or("");

    // Combine host + path, replace non-alphanumeric runs with single hyphens
    let combined = format!("{}{}", host, path);
    let slug: String = combined
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();

    // Collapse runs of hyphens and trim
    let slug = slug
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    slug.to_lowercase()
}

// ---------------------------------------------------------------------------
// Lookup
// ---------------------------------------------------------------------------

/// Look up a cached article by URL. Returns `None` on a cache miss.
pub fn lookup(url: &str) -> Result<Option<CachedArticle>> {
    lookup_in(&cache_dir()?, url)
}

fn lookup_in(base: &PathBuf, url: &str) -> Result<Option<CachedArticle>> {
    let dir = base.join(url_to_slug(url));
    let article_md = dir.join("article.md");
    let article_txt = dir.join("article.txt");

    if !article_md.exists() || !article_txt.exists() {
        return Ok(None);
    }

    let src = std::fs::read_to_string(&article_md)
        .with_context(|| format!("failed to read {}", article_md.display()))?;

    let (fm, body) = frontmatter::parse::<ArticleFrontmatter>(&src)
        .with_context(|| format!("failed to parse {}", article_md.display()))?;

    let plain_text = std::fs::read_to_string(&article_txt)
        .with_context(|| format!("failed to read {}", article_txt.display()))?;

    Ok(Some(CachedArticle {
        url: fm.url,
        title: fm.title,
        authors: fm.authors,
        date: fm.date,
        description: fm.description,
        content: body.to_string(),
        plain_text,
    }))
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// Write an article to the cache, overwriting any existing entry for this URL.
pub fn store(
    url: &str,
    title: Option<&str>,
    authors: &[String],
    date: Option<&str>,
    description: Option<&str>,
    content: &str,
    plain_text: &str,
) -> Result<PathBuf> {
    store_in(&cache_dir()?, url, title, authors, date, description, content, plain_text)
}

fn store_in(
    base: &PathBuf,
    url: &str,
    title: Option<&str>,
    authors: &[String],
    date: Option<&str>,
    description: Option<&str>,
    content: &str,
    plain_text: &str,
) -> Result<PathBuf> {
    let dir = base.join(url_to_slug(url));
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create {}", dir.display()))?;

    let fm = ArticleFrontmatter {
        url: url.to_string(),
        title: title.map(str::to_string),
        authors: authors.to_vec(),
        date: date.map(str::to_string),
        description: description.map(str::to_string),
        cached_at: Utc::now().to_rfc3339(),
    };
    let yaml = serde_yaml::to_string(&fm)
        .context("failed to serialize frontmatter")?;
    let md_path = dir.join("article.md");
    std::fs::write(&md_path, format!("---\n{}---\n{}", yaml, content))
        .with_context(|| format!("failed to write {}", md_path.display()))?;

    let txt_path = dir.join("article.txt");
    std::fs::write(&txt_path, plain_text)
        .with_context(|| format!("failed to write {}", txt_path.display()))?;

    Ok(dir)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_to_slug_basic() {
        assert_eq!(
            url_to_slug("https://ultrasaurus.com/2015/10/software-isnt-real/"),
            "ultrasaurus-com-2015-10-software-isnt-real"
        );
    }

    #[test]
    fn url_to_slug_no_path() {
        assert_eq!(url_to_slug("https://example.com"), "example-com");
    }

    #[test]
    fn url_to_slug_with_query() {
        // query string is not included in path slug
        assert_eq!(
            url_to_slug("https://example.com/page?q=1"),
            "example-com-page"
        );
    }

    #[test]
    fn url_to_slug_collapses_hyphens() {
        assert_eq!(
            url_to_slug("https://example.com/a--b/c"),
            "example-com-a-b-c"
        );
    }

    #[test]
    fn store_and_lookup_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        let url = "https://example.com/test-article/";

        store_in(
            &base, url,
            Some("Test Article"),
            &["Alice".to_string()],
            Some("2024-01-15"),
            Some("A test."),
            "# Test\n\nMarkdown body.",
            "Test\n\nPlain text body.",
        ).unwrap();

        let hit = lookup_in(&base, url).unwrap().expect("should be a cache hit");
        assert_eq!(hit.url, url);
        assert_eq!(hit.title.as_deref(), Some("Test Article"));
        assert_eq!(hit.authors, vec!["Alice"]);
        assert_eq!(hit.date.as_deref(), Some("2024-01-15"));
        assert_eq!(hit.content, "# Test\n\nMarkdown body.");
        assert_eq!(hit.plain_text, "Test\n\nPlain text body.");
    }

    #[test]
    fn lookup_miss_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        let result = lookup_in(&base, "https://example.com/not-cached/").unwrap();
        assert!(result.is_none());
    }
}
