//! Article cache — stores fetched articles in `~/.odoru/articles/`.
//!
//! Each cached article lives in its own directory named after the URL:
//!   `~/.odoru/articles/<hostname>-<slugified-path>/`
//!     - `article.md`  — YAML frontmatter (metadata) + markdown body
//!     - `article.txt` — plain text (no frontmatter)
//!
//! Lookup is by URL: scan directories, match the `url` frontmatter field.
//! This is fast enough for a personal collection.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use tracing::warn;
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
    /// Voice IDs (e.g. "f5:sarah") for which all sentences are synthesized.
    /// Populated lazily on GET /doc so subsequent calls skip the audio check.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    synthesized_voices: Vec<String>,
    /// Total audio duration in seconds, keyed by voice ID.
    /// Written when a background job completes synthesis.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    voice_durations: HashMap<String, f64>,
    /// If true, include this article in the next static export.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    publish: bool,
    /// Voice ID to use for the exported audio. Only meaningful when publish is true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    published_voice: Option<String>,
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
    /// RFC 3339 timestamp when the article was fetched and cached.
    pub cached_at: String,
    /// Markdown content.
    pub content: String,
    /// Plain text content (for TTS).
    pub plain_text: String,
    /// Voice IDs for which synthesis is complete (e.g. "f5:sarah").
    pub synthesized_voices: Vec<String>,
    /// Total audio duration in seconds, keyed by voice ID.
    pub voice_durations: HashMap<String, f64>,
    /// If true, include this article in the next static export.
    pub publish: bool,
    /// Voice ID to use for the exported audio. Only meaningful when publish is true.
    pub published_voice: Option<String>,
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
        cached_at: fm.cached_at,
        content: body.to_string(),
        plain_text,
        synthesized_voices: fm.synthesized_voices,
        voice_durations: fm.voice_durations,
        publish: fm.publish,
        published_voice: fm.published_voice,
    }))
}

// ---------------------------------------------------------------------------
// List all
// ---------------------------------------------------------------------------

/// Return metadata for every cached article. Skips unreadable entries silently.
/// Does not read `content` — metadata only.
pub fn list_all() -> Result<Vec<CachedArticle>> {
    list_all_in(&cache_dir()?)
}

fn list_all_in(base: &PathBuf) -> Result<Vec<CachedArticle>> {
    let mut articles = Vec::new();
    let entries = match std::fs::read_dir(base) {
        Ok(e) => e,
        Err(_) => return Ok(articles),
    };
    for entry in entries {
        let path = match entry { Ok(e) => e.path(), Err(_) => continue };
        let md_path = path.join("article.md");
        let txt_path = path.join("article.txt");
        if !md_path.exists() { continue; }
        let src = match std::fs::read_to_string(&md_path) {
            Ok(s) => s,
            Err(e) => { warn!("Skipping unreadable article {}: {e}", md_path.display()); continue; }
        };
        let (fm, _body) = match frontmatter::parse::<ArticleFrontmatter>(&src) {
            Ok(p) => p,
            Err(e) => { warn!("Skipping unparseable article {}: {e}", md_path.display()); continue; }
        };
        articles.push(CachedArticle {
            url: fm.url,
            title: fm.title,
            authors: fm.authors,
            date: fm.date,
            description: fm.description,
            cached_at: fm.cached_at,
            // content is not read for the list endpoint — callers that need
            // body text should call lookup() directly.
            content: String::new(),
            plain_text: if txt_path.exists() {
                std::fs::read_to_string(&txt_path).unwrap_or_default()
            } else {
                String::new()
            },
            synthesized_voices: fm.synthesized_voices,
            voice_durations: fm.voice_durations,
            publish: fm.publish,
            published_voice: fm.published_voice,
        });
    }
    Ok(articles)
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
        synthesized_voices: Vec::new(),
        voice_durations: HashMap::new(),
        publish: false,
        published_voice: None,
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
// Update publish settings
// ---------------------------------------------------------------------------

/// Set the `publish` flag and `published_voice` for a cached article.
/// No-ops if the article is not cached. Intended to be called from `spawn_blocking`.
pub fn update_publish(url: &str, publish: bool, published_voice: Option<&str>) -> Result<()> {
    update_publish_in(&cache_dir()?, url, publish, published_voice)
}

fn update_publish_in(base: &PathBuf, url: &str, publish: bool, published_voice: Option<&str>) -> Result<()> {
    let dir = base.join(url_to_slug(url));
    let md_path = dir.join("article.md");
    if !md_path.exists() { return Ok(()); }

    let src = std::fs::read_to_string(&md_path)
        .with_context(|| format!("failed to read {}", md_path.display()))?;
    let (mut fm, body) = frontmatter::parse::<ArticleFrontmatter>(&src)
        .with_context(|| format!("failed to parse {}", md_path.display()))?;

    fm.publish = publish;
    fm.published_voice = published_voice.map(str::to_string);

    let yaml = serde_yaml::to_string(&fm)
        .context("failed to serialize frontmatter")?;
    std::fs::write(&md_path, format!("---\n{}---\n{}", yaml, body))
        .with_context(|| format!("failed to write {}", md_path.display()))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Mark synthesized
// ---------------------------------------------------------------------------

/// Record that `voice_id` (e.g. "f5:sarah") is fully synthesized for `url`,
/// storing the total audio `duration_secs`.
///
/// Reads the existing `article.md`, adds the voice to `synthesized_voices`
/// and its duration to `voice_durations`, then rewrites the file. No-ops if
/// not cached. Intended to be called from `spawn_blocking`.
pub fn mark_synthesized(url: &str, voice_id: &str, duration_secs: f64) -> Result<()> {
    mark_synthesized_in(&cache_dir()?, url, voice_id, duration_secs)
}

fn mark_synthesized_in(base: &PathBuf, url: &str, voice_id: &str, duration_secs: f64) -> Result<()> {
    let dir = base.join(url_to_slug(url));
    let md_path = dir.join("article.md");
    if !md_path.exists() { return Ok(()); }

    let src = std::fs::read_to_string(&md_path)
        .with_context(|| format!("failed to read {}", md_path.display()))?;

    let (mut fm, body) = frontmatter::parse::<ArticleFrontmatter>(&src)
        .with_context(|| format!("failed to parse {}", md_path.display()))?;

    if !fm.synthesized_voices.iter().any(|v| v == voice_id) {
        fm.synthesized_voices.push(voice_id.to_string());
    }
    fm.voice_durations.insert(voice_id.to_string(), duration_secs);

    let yaml = serde_yaml::to_string(&fm)
        .context("failed to serialize frontmatter")?;
    std::fs::write(&md_path, format!("---\n{}---\n{}", yaml, body))
        .with_context(|| format!("failed to write {}", md_path.display()))?;

    Ok(())
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
        assert!(!hit.cached_at.is_empty(), "cached_at should be set");
    }

    #[test]
    fn publish_defaults_to_false_on_new_article() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        let url = "https://example.com/pub-test/";
        store_in(&base, url, Some("T"), &[], None, None, "#", "plain").unwrap();
        let hit = lookup_in(&base, url).unwrap().unwrap();
        assert!(!hit.publish);
        assert_eq!(hit.published_voice, None);
    }

    #[test]
    fn update_publish_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        let url = "https://example.com/pub-update/";
        store_in(&base, url, Some("T"), &[], None, None, "#", "plain").unwrap();

        update_publish_in(&base, url, true, Some("kokoro:af_heart")).unwrap();

        let hit = lookup_in(&base, url).unwrap().unwrap();
        assert!(hit.publish);
        assert_eq!(hit.published_voice.as_deref(), Some("kokoro:af_heart"));
    }

    #[test]
    fn update_publish_can_unset() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        let url = "https://example.com/pub-unset/";
        store_in(&base, url, Some("T"), &[], None, None, "#", "plain").unwrap();
        update_publish_in(&base, url, true, Some("kokoro:af_heart")).unwrap();
        update_publish_in(&base, url, false, None).unwrap();
        let hit = lookup_in(&base, url).unwrap().unwrap();
        assert!(!hit.publish);
        assert_eq!(hit.published_voice, None);
    }

    #[test]
    fn old_article_without_publish_fields_loads_cleanly() {
        // Simulate an article.md written before publish fields existed.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        let url = "https://example.com/old-article/";
        // Write a minimal frontmatter without publish/published_voice.
        let slug = url_to_slug(url);
        let dir = base.join(&slug);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("article.md"),
            "---\nurl: 'https://example.com/old-article/'\ncached_at: '2024-01-01T00:00:00Z'\n---\nbody"
        ).unwrap();
        std::fs::write(dir.join("article.txt"), "plain").unwrap();
        let hit = lookup_in(&base, url).unwrap().unwrap();
        assert!(!hit.publish);
        assert_eq!(hit.published_voice, None);
    }

    #[test]
    fn lookup_miss_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        let result = lookup_in(&base, "https://example.com/not-cached/").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn list_all_returns_all_articles() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();

        store_in(&base, "https://example.com/a/", Some("Article A"), &[], None, None,
            "# A", "Plain A").unwrap();
        store_in(&base, "https://example.com/b/", Some("Article B"), &[], None, None,
            "# B", "Plain B").unwrap();

        let mut articles = list_all_in(&base).unwrap();
        articles.sort_by(|a, b| a.url.cmp(&b.url));

        assert_eq!(articles.len(), 2);
        assert_eq!(articles[0].title.as_deref(), Some("Article A"));
        assert_eq!(articles[1].title.as_deref(), Some("Article B"));
        assert!(!articles[0].cached_at.is_empty());
        assert!(!articles[1].cached_at.is_empty());
    }

    #[test]
    fn list_all_skips_unreadable_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();

        // A valid article.
        store_in(&base, "https://example.com/good/", Some("Good"), &[], None, None,
            "# Good", "Good plain").unwrap();

        // A directory with no article.md — should be silently skipped.
        std::fs::create_dir_all(base.join("junk-dir")).unwrap();

        let articles = list_all_in(&base).unwrap();
        assert_eq!(articles.len(), 1);
        assert_eq!(articles[0].title.as_deref(), Some("Good"));
    }

    #[test]
    fn list_all_includes_synthesized_voices() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();

        store_in(&base, "https://example.com/voiced/", Some("Voiced"), &[], None, None,
            "# V", "Plain V").unwrap();
        mark_synthesized_in(&base, "https://example.com/voiced/", "f5:sarah", 42.0).unwrap();

        let articles = list_all_in(&base).unwrap();
        assert_eq!(articles.len(), 1);
        assert_eq!(articles[0].synthesized_voices, vec!["f5:sarah"]);
        assert_eq!(articles[0].voice_durations.get("f5:sarah").copied(), Some(42.0));
    }
}
