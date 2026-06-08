//! UUID-keyed document store — `~/.odoru/documents/<uuid>/`.
//!
//! Each document directory contains:
//!   `document.md`  — YAML frontmatter + markdown body
//!   `document.txt` — plain text for TTS
//!   `source.html`  — originally fetched HTML (content hash source; display deferred)
//!   `voices.json`  — per-voice synthesis state
//!
//! This module handles all disk I/O for documents. Index management
//! (source_url → uuid, content_hash → uuid) lives in `index.rs`.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::frontmatter;

// ---------------------------------------------------------------------------
// Fetch status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FetchStatus {
    Fetching,
    Ready,
    Error,
}

// ---------------------------------------------------------------------------
// Voice state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceStatus {
    InProgress,
    Ready,
    Stale,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceState {
    pub status: VoiceStatus,
    /// Present once ever synthesized; survives stale transition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    /// Job ID — used to detect re-trigger of in_progress voice.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    /// True if this is the author's chosen voice for publication.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub published: bool,
}

pub type VoicesMap = HashMap<String, VoiceState>;

// ---------------------------------------------------------------------------
// Frontmatter schema
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct DocumentFrontmatter {
    /// Stable UUID assigned at creation.
    id: String,
    /// Fetch status — written immediately on creation, updated on fetch completion.
    status: FetchStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default)]
    authors: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cached_at: Option<String>,
    /// Whether this document's text is published.
    #[serde(default)]
    publish: bool,
    /// SHA-256 of originally fetched HTML. Stored for index rebuild.
    #[serde(skip_serializing_if = "Option::is_none")]
    content_hash: Option<String>,
}

// ---------------------------------------------------------------------------
// Public document type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Document {
    pub id: String,
    pub status: FetchStatus,
    pub source_url: Option<String>,
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub date: Option<String>,
    pub description: Option<String>,
    pub cached_at: Option<String>,
    pub publish: bool,
    pub content_hash: Option<String>,
    /// Markdown content (empty for fetching/error status).
    pub content: String,
    /// Plain text for TTS (empty for fetching/error status).
    pub plain_text: String,
    /// Per-voice synthesis state.
    pub voices: VoicesMap,
    /// Set when voices.json failed to parse; voices will be empty.
    pub voices_error: Option<String>,
}

// ---------------------------------------------------------------------------
// Export metadata
// ---------------------------------------------------------------------------

/// Metadata for a document included in a static export.
///
/// Always constructible from a `Document` — `voice_id` is `None` when no voice
/// has been marked published. The CLI warns in that case but still exports text.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExportMeta {
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub date: Option<String>,
    pub source_url: Option<String>,
    /// Voice ID of the published voice (e.g. `"f5:sarah"`), if set.
    pub voice_id: Option<String>,
}

impl Document {
    /// Build export metadata from this document.
    ///
    /// `voice_id` is `None` when no voice has `published: true`.
    pub fn export_meta(&self) -> ExportMeta {
        let voice_id = self.voices.iter()
            .find(|(_, v)| v.published)
            .map(|(id, _)| id.clone());
        ExportMeta {
            title: self.title.clone(),
            authors: self.authors.clone(),
            date: self.date.clone(),
            source_url: self.source_url.clone(),
            voice_id,
        }
    }
}

// ---------------------------------------------------------------------------
// Directory helpers
// ---------------------------------------------------------------------------

/// Returns `~/.odoru/documents/`, creating it if needed.
pub fn documents_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("$HOME not set")?;
    let dir = PathBuf::from(home).join(".odoru").join("documents");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create documents dir {}", dir.display()))?;
    Ok(dir)
}

fn doc_dir(base: &PathBuf, id: &str) -> PathBuf {
    base.join(id)
}

// ---------------------------------------------------------------------------
// Create (status: fetching — before fetch completes)
// ---------------------------------------------------------------------------

/// Create a new document record with `status: fetching`. Returns the UUID.
/// Call `store_ready` once the fetch completes.
pub fn create_fetching(source_url: Option<&str>) -> Result<String> {
    create_fetching_in(&documents_dir()?, source_url)
}

pub fn create_fetching_in(base: &PathBuf, source_url: Option<&str>) -> Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let dir = doc_dir(base, &id);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create document dir {}", dir.display()))?;

    let fm = DocumentFrontmatter {
        id: id.clone(),
        status: FetchStatus::Fetching,
        source_url: source_url.map(str::to_string),
        title: None,
        authors: vec![],
        date: None,
        description: None,
        cached_at: None,
        publish: false,
        content_hash: None,
    };
    write_frontmatter(&dir, &fm, "")?;

    // Empty voices.json
    let voices_path = dir.join("voices.json");
    std::fs::write(&voices_path, "{}")
        .with_context(|| format!("failed to write {}", voices_path.display()))?;

    Ok(id)
}

// ---------------------------------------------------------------------------
// Create ready (direct text — no URL fetch)
// ---------------------------------------------------------------------------

/// Create a document directly from content without fetching a URL. Returns the UUID.
pub fn create_ready(
    title: Option<&str>,
    content: &str,
    plain_text: &str,
    content_hash: &str,
) -> Result<String> {
    create_ready_in(&documents_dir()?, title, content, plain_text, content_hash)
}

pub fn create_ready_in(
    base: &PathBuf,
    title: Option<&str>,
    content: &str,
    plain_text: &str,
    content_hash: &str,
) -> Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let dir = doc_dir(base, &id);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create document dir {}", dir.display()))?;

    let fm = DocumentFrontmatter {
        id: id.clone(),
        status: FetchStatus::Ready,
        source_url: None,
        title: title.map(str::to_string),
        authors: vec![],
        date: None,
        description: None,
        cached_at: Some(Utc::now().to_rfc3339()),
        publish: false,
        content_hash: Some(content_hash.to_string()),
    };
    write_frontmatter(&dir, &fm, content)?;

    let txt_path = dir.join("document.txt");
    std::fs::write(&txt_path, plain_text)
        .with_context(|| format!("failed to write {}", txt_path.display()))?;

    let voices_path = dir.join("voices.json");
    std::fs::write(&voices_path, "{}")
        .with_context(|| format!("failed to write {}", voices_path.display()))?;

    Ok(id)
}

// ---------------------------------------------------------------------------
// Store ready (fetch completed)
// ---------------------------------------------------------------------------

/// Write a fully fetched document. Updates status to `ready`.
#[allow(clippy::too_many_arguments)]
pub fn store_ready(
    id: &str,
    source_url: Option<&str>,
    title: Option<&str>,
    authors: &[String],
    date: Option<&str>,
    description: Option<&str>,
    content: &str,
    plain_text: &str,
    source_html: &str,
    content_hash: &str,
) -> Result<()> {
    store_ready_in(
        &documents_dir()?, id, source_url, title, authors, date, description,
        content, plain_text, source_html, content_hash,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn store_ready_in(
    base: &PathBuf,
    id: &str,
    source_url: Option<&str>,
    title: Option<&str>,
    authors: &[String],
    date: Option<&str>,
    description: Option<&str>,
    content: &str,
    plain_text: &str,
    source_html: &str,
    content_hash: &str,
) -> Result<()> {
    let dir = doc_dir(base, id);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create document dir {}", dir.display()))?;

    let fm = DocumentFrontmatter {
        id: id.to_string(),
        status: FetchStatus::Ready,
        source_url: source_url.map(str::to_string),
        title: title.map(str::to_string),
        authors: authors.to_vec(),
        date: date.map(str::to_string),
        description: description.map(str::to_string),
        cached_at: Some(Utc::now().to_rfc3339()),
        publish: false,
        content_hash: Some(content_hash.to_string()),
    };
    write_frontmatter(&dir, &fm, content)?;

    let txt_path = dir.join("document.txt");
    std::fs::write(&txt_path, plain_text)
        .with_context(|| format!("failed to write {}", txt_path.display()))?;

    let html_path = dir.join("source.html");
    std::fs::write(&html_path, source_html)
        .with_context(|| format!("failed to write {}", html_path.display()))?;

    // Create voices.json only if it doesn't already exist (preserve any
    // in_progress state from a concurrent WS session).
    let voices_path = dir.join("voices.json");
    if !voices_path.exists() {
        std::fs::write(&voices_path, "{}")
            .with_context(|| format!("failed to write {}", voices_path.display()))?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Store error
// ---------------------------------------------------------------------------

/// Mark a fetching document as errored.
pub fn store_error(id: &str, error_msg: &str) -> Result<()> {
    store_error_in(&documents_dir()?, id, error_msg)
}

pub fn store_error_in(base: &PathBuf, id: &str, error_msg: &str) -> Result<()> {
    let dir = doc_dir(base, id);
    let md_path = dir.join("document.md");
    if !md_path.exists() { return Ok(()); }

    let src = std::fs::read_to_string(&md_path)
        .with_context(|| format!("failed to read {}", md_path.display()))?;
    let (mut fm, _body) = frontmatter::parse::<DocumentFrontmatter>(&src)
        .with_context(|| format!("failed to parse {}", md_path.display()))?;

    fm.status = FetchStatus::Error;
    // Store error message in description for now (visible in GET /documents/:id).
    fm.description = Some(error_msg.to_string());
    write_frontmatter(&dir, &fm, "")?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Lookup by ID
// ---------------------------------------------------------------------------

/// Look up a document by UUID. Returns `None` if not found.
pub fn lookup_by_id(id: &str) -> Result<Option<Document>> {
    lookup_by_id_in(&documents_dir()?, id)
}

pub fn lookup_by_id_in(base: &PathBuf, id: &str) -> Result<Option<Document>> {
    let dir = doc_dir(base, id);
    let md_path = dir.join("document.md");
    if !md_path.exists() {
        return Ok(None);
    }

    let src = std::fs::read_to_string(&md_path)
        .with_context(|| format!("failed to read {}", md_path.display()))?;
    let (fm, body) = frontmatter::parse::<DocumentFrontmatter>(&src)
        .with_context(|| format!("failed to parse {}", md_path.display()))?;

    let txt_path = dir.join("document.txt");
    let plain_text = if txt_path.exists() {
        std::fs::read_to_string(&txt_path)
            .with_context(|| format!("failed to read {}", txt_path.display()))?
    } else {
        String::new()
    };

    let (voices, voices_error) = match read_voices_in(&dir) {
        Ok(v) => (v, None),
        Err(e) => {
            warn!("failed to parse voices.json for {}: {e}", dir.display());
            (HashMap::new(), Some(e.to_string()))
        }
    };

    Ok(Some(Document {
        id: fm.id,
        status: fm.status,
        source_url: fm.source_url,
        title: fm.title,
        authors: fm.authors,
        date: fm.date,
        description: fm.description,
        cached_at: fm.cached_at,
        publish: fm.publish,
        content_hash: fm.content_hash,
        content: body.to_string(),
        plain_text,
        voices,
        voices_error,
    }))
}

// ---------------------------------------------------------------------------
// List all
// ---------------------------------------------------------------------------

/// Return all documents (metadata + voices only, no content/plain_text).
pub fn list_all() -> Result<Vec<Document>> {
    list_all_in(&documents_dir()?)
}

pub fn list_all_in(base: &PathBuf) -> Result<Vec<Document>> {
    let mut docs = Vec::new();
    let entries = match std::fs::read_dir(base) {
        Ok(e) => e,
        Err(_) => return Ok(docs),
    };
    for entry in entries {
        let path = match entry { Ok(e) => e.path(), Err(_) => continue };
        let md_path = path.join("document.md");
        if !md_path.exists() { continue; }
        let src = match std::fs::read_to_string(&md_path) {
            Ok(s) => s,
            Err(e) => { warn!("Skipping unreadable {}: {e}", md_path.display()); continue; }
        };
        let (fm, _body) = match frontmatter::parse::<DocumentFrontmatter>(&src) {
            Ok(p) => p,
            Err(e) => { warn!("Skipping unparseable {}: {e}", md_path.display()); continue; }
        };
        let voices = read_voices_in(&path).unwrap_or_default();
        docs.push(Document {
            id: fm.id,
            status: fm.status,
            source_url: fm.source_url,
            title: fm.title,
            authors: fm.authors,
            date: fm.date,
            description: fm.description,
            cached_at: fm.cached_at,
            publish: fm.publish,
            content_hash: fm.content_hash,
            content: String::new(),
            plain_text: String::new(),
            voices,
            voices_error: None,
        });
    }
    Ok(docs)
}

// ---------------------------------------------------------------------------
// Update publish settings
// ---------------------------------------------------------------------------

/// Update `publish` flag and optionally set `published: true` on one voice
/// (clearing others). Pass `None` for `published_voice` to leave voices unchanged.
pub fn update_publish(id: &str, publish: bool, published_voice: Option<&str>) -> Result<()> {
    update_publish_in(&documents_dir()?, id, publish, published_voice)
}

pub fn update_publish_in(
    base: &PathBuf,
    id: &str,
    publish: bool,
    published_voice: Option<&str>,
) -> Result<()> {
    let dir = doc_dir(base, id);
    let md_path = dir.join("document.md");
    if !md_path.exists() { return Ok(()); }

    let src = std::fs::read_to_string(&md_path)
        .with_context(|| format!("failed to read {}", md_path.display()))?;
    let (mut fm, body) = frontmatter::parse::<DocumentFrontmatter>(&src)
        .with_context(|| format!("failed to parse {}", md_path.display()))?;

    fm.publish = publish;
    write_frontmatter(&dir, &fm, body)?;

    if let Some(voice_id) = published_voice {
        let mut voices = read_voices_in(&dir)?;
        // Clear published flag on all voices, set on the chosen one.
        for (k, v) in voices.iter_mut() {
            v.published = k == voice_id;
        }
        // If the voice doesn't exist yet, create a minimal entry.
        if !voices.contains_key(voice_id) {
            voices.insert(voice_id.to_string(), VoiceState {
                status: VoiceStatus::InProgress,
                duration: None,
                job_id: None,
                published: true,
            });
        }
        write_voices_in(&dir, &voices)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Metadata editing
// ---------------------------------------------------------------------------

/// Update a document's title, authors, and date fields.
pub fn update_metadata(id: &str, title: Option<&str>, authors: &[String], date: Option<&str>) -> Result<()> {
    update_metadata_in(&documents_dir()?, id, title, authors, date)
}

pub fn update_metadata_in(
    base: &PathBuf,
    id: &str,
    title: Option<&str>,
    authors: &[String],
    date: Option<&str>,
) -> Result<()> {
    let dir = doc_dir(base, id);
    let md_path = dir.join("document.md");
    if !md_path.exists() { return Ok(()); }

    let src = std::fs::read_to_string(&md_path)
        .with_context(|| format!("failed to read {}", md_path.display()))?;
    let (mut fm, body) = frontmatter::parse::<DocumentFrontmatter>(&src)
        .with_context(|| format!("failed to parse {}", md_path.display()))?;

    fm.title = title.map(str::to_string);
    fm.authors = authors.to_vec();
    fm.date = date.map(str::to_string);
    write_frontmatter(&dir, &fm, body)
}

// ---------------------------------------------------------------------------
// Content editing
// ---------------------------------------------------------------------------

/// Update a document's content and plain_text, marking all synthesized voices stale.
pub fn update_content(id: &str, content: &str, plain_text: &str) -> Result<()> {
    update_content_in(&documents_dir()?, id, content, plain_text)
}

pub fn update_content_in(base: &PathBuf, id: &str, content: &str, plain_text: &str) -> Result<()> {
    let dir = doc_dir(base, id);
    let md_path = dir.join("document.md");
    if !md_path.exists() {
        return Err(anyhow::anyhow!("document not found: {id}"));
    }

    let src = std::fs::read_to_string(&md_path)
        .with_context(|| format!("failed to read {}", md_path.display()))?;
    let (fm, _old_body) = frontmatter::parse::<DocumentFrontmatter>(&src)
        .with_context(|| format!("failed to parse {}", md_path.display()))?;
    write_frontmatter(&dir, &fm, content)?;

    let txt_path = dir.join("document.txt");
    std::fs::write(&txt_path, plain_text)
        .with_context(|| format!("failed to write {}", txt_path.display()))?;

    // Mark all ready/in_progress voices stale — old audio is still playable.
    let mut voices = read_voices_in(&dir)?;
    for v in voices.values_mut() {
        if matches!(v.status, VoiceStatus::Ready | VoiceStatus::InProgress) {
            v.status = VoiceStatus::Stale;
        }
    }
    write_voices_in(&dir, &voices)
}

// ---------------------------------------------------------------------------
// Delete
// ---------------------------------------------------------------------------

pub fn delete_document(id: &str) -> Result<()> {
    delete_document_in(&documents_dir()?, id)
}

pub fn delete_document_in(base: &PathBuf, id: &str) -> Result<()> {
    let dir = doc_dir(base, id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .with_context(|| format!("failed to remove {}", dir.display()))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Voice state helpers
// ---------------------------------------------------------------------------

/// Read `voices.json` for a document directory.
pub fn read_voices(id: &str) -> Result<VoicesMap> {
    read_voices_in(&doc_dir(&documents_dir()?, id))
}

pub fn read_voices_in(dir: &PathBuf) -> Result<VoicesMap> {
    let path = dir.join("voices.json");
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let src = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&src)
        .with_context(|| format!("failed to parse {}", path.display()))
}

/// Write `voices.json` atomically (write to temp, then rename).
pub fn write_voices(id: &str, voices: &VoicesMap) -> Result<()> {
    write_voices_in(&doc_dir(&documents_dir()?, id), voices)
}

pub fn write_voices_in(dir: &PathBuf, voices: &VoicesMap) -> Result<()> {
    let path = dir.join("voices.json");
    let tmp_path = dir.join("voices.json.tmp");
    let json = serde_json::to_string_pretty(voices)
        .context("failed to serialize voices")?;
    std::fs::write(&tmp_path, &json)
        .with_context(|| format!("failed to write {}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, &path)
        .with_context(|| format!("failed to rename {} → {}", tmp_path.display(), path.display()))?;
    Ok(())
}

/// Update a single voice's state, creating the entry if it doesn't exist.
pub fn update_voice_status(
    id: &str,
    voice_id: &str,
    status: VoiceStatus,
    duration: Option<f64>,
    job_id: Option<&str>,
) -> Result<()> {
    update_voice_status_in(
        &doc_dir(&documents_dir()?, id),
        voice_id, status, duration, job_id,
    )
}

pub fn update_voice_status_in(
    dir: &PathBuf,
    voice_id: &str,
    status: VoiceStatus,
    duration: Option<f64>,
    job_id: Option<&str>,
) -> Result<()> {
    let mut voices = read_voices_in(dir)?;
    let entry = voices.entry(voice_id.to_string()).or_insert_with(|| VoiceState {
        status: VoiceStatus::InProgress,
        duration: None,
        job_id: None,
        published: false,
    });
    entry.status = status;
    if let Some(d) = duration {
        entry.duration = Some(d);
    }
    if let Some(jid) = job_id {
        entry.job_id = Some(jid.to_string());
    }
    write_voices_in(dir, &voices)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn write_frontmatter(dir: &PathBuf, fm: &DocumentFrontmatter, body: &str) -> Result<()> {
    let md_path = dir.join("document.md");
    let tmp_path = dir.join("document.md.tmp");
    let yaml = serde_yaml::to_string(fm).context("failed to serialize frontmatter")?;
    let content = format!("---\n{}---\n{}", yaml, body);
    std::fs::write(&tmp_path, &content)
        .with_context(|| format!("failed to write {}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, &md_path)
        .with_context(|| format!("failed to rename {} → {}", tmp_path.display(), md_path.display()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_fetching_and_lookup() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();

        let id = create_fetching_in(&base, Some("https://example.com/article/")).unwrap();
        let doc = lookup_by_id_in(&base, &id).unwrap().expect("should exist");
        assert_eq!(doc.status, FetchStatus::Fetching);
        assert_eq!(doc.source_url.as_deref(), Some("https://example.com/article/"));
        assert!(doc.content.is_empty());
        assert!(doc.voices.is_empty());
    }

    #[test]
    fn store_ready_and_lookup() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();

        let id = create_fetching_in(&base, Some("https://example.com/a/")).unwrap();
        store_ready_in(
            &base, &id,
            Some("https://example.com/a/"),
            Some("Test Article"),
            &["Alice".to_string()],
            Some("2024-01-15"),
            Some("A test."),
            "# Test\n\nBody.",
            "Test Body.",
            "<html>raw</html>",
            "abc123hash",
        ).unwrap();

        let doc = lookup_by_id_in(&base, &id).unwrap().expect("should exist");
        assert_eq!(doc.status, FetchStatus::Ready);
        assert_eq!(doc.title.as_deref(), Some("Test Article"));
        assert_eq!(doc.content, "# Test\n\nBody.");
        assert_eq!(doc.plain_text, "Test Body.");
        assert_eq!(doc.content_hash.as_deref(), Some("abc123hash"));
    }

    #[test]
    fn store_error_updates_status() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();

        let id = create_fetching_in(&base, Some("https://example.com/b/")).unwrap();
        store_error_in(&base, &id, "connection refused").unwrap();

        let doc = lookup_by_id_in(&base, &id).unwrap().expect("should exist");
        assert_eq!(doc.status, FetchStatus::Error);
    }

    #[test]
    fn voices_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();

        let id = create_fetching_in(&base, None).unwrap();
        let dir = base.join(&id);

        update_voice_status_in(&dir, "f5:sarah", VoiceStatus::Ready, Some(120.5), Some("job-1")).unwrap();

        let voices = read_voices_in(&dir).unwrap();
        let v = voices.get("f5:sarah").unwrap();
        assert_eq!(v.status, VoiceStatus::Ready);
        assert_eq!(v.duration, Some(120.5));
        assert_eq!(v.job_id.as_deref(), Some("job-1"));
    }

    #[test]
    fn list_all_returns_docs() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();

        let id1 = create_fetching_in(&base, Some("https://example.com/a/")).unwrap();
        let id2 = create_fetching_in(&base, Some("https://example.com/b/")).unwrap();

        let docs = list_all_in(&base).unwrap();
        assert_eq!(docs.len(), 2);
        let ids: Vec<_> = docs.iter().map(|d| d.id.clone()).collect();
        assert!(ids.contains(&id1));
        assert!(ids.contains(&id2));
    }

    #[test]
    fn lookup_miss_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        assert!(lookup_by_id_in(&base, "nonexistent-uuid").unwrap().is_none());
    }

    #[test]
    fn publish_defaults_to_false_on_new_document() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        let id = create_fetching_in(&base, Some("https://example.com/pub-test/")).unwrap();
        let doc = lookup_by_id_in(&base, &id).unwrap().unwrap();
        assert!(!doc.publish);
    }

    #[test]
    fn update_publish_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        let id = create_fetching_in(&base, Some("https://example.com/pub-update/")).unwrap();

        update_publish_in(&base, &id, true, Some("kokoro:af_heart")).unwrap();

        let doc = lookup_by_id_in(&base, &id).unwrap().unwrap();
        assert!(doc.publish);
        let voices = read_voices_in(&base.join(&id)).unwrap();
        assert!(voices.get("kokoro:af_heart").map(|v| v.published).unwrap_or(false));
    }

    #[test]
    fn update_publish_can_unset() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        let id = create_fetching_in(&base, Some("https://example.com/pub-unset/")).unwrap();
        update_publish_in(&base, &id, true, Some("kokoro:af_heart")).unwrap();
        update_publish_in(&base, &id, false, None).unwrap();
        let doc = lookup_by_id_in(&base, &id).unwrap().unwrap();
        assert!(!doc.publish);
        // published flag on voice should be unchanged (we passed None for published_voice)
        let voices = read_voices_in(&base.join(&id)).unwrap();
        assert!(voices.get("kokoro:af_heart").map(|v| v.published).unwrap_or(false));
    }

    #[test]
    fn list_all_skips_unreadable_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();

        create_fetching_in(&base, Some("https://example.com/good/")).unwrap();
        // A directory with no document.md — should be silently skipped.
        std::fs::create_dir_all(base.join("junk-dir")).unwrap();

        let docs = list_all_in(&base).unwrap();
        assert_eq!(docs.len(), 1);
    }

    #[test]
    fn list_all_includes_voice_state() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();

        let id = create_fetching_in(&base, Some("https://example.com/voiced/")).unwrap();
        let dir = base.join(&id);
        update_voice_status_in(&dir, "f5:sarah", VoiceStatus::Ready, Some(42.0), Some("job-1")).unwrap();

        let docs = list_all_in(&base).unwrap();
        assert_eq!(docs.len(), 1);
        let v = docs[0].voices.get("f5:sarah").unwrap();
        assert_eq!(v.status, VoiceStatus::Ready);
        assert_eq!(v.duration, Some(42.0));
    }
}
