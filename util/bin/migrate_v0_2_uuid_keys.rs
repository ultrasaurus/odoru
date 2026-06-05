//! Migration: v0.1 URL-slug article directories → v0.2 UUID-keyed directories.
//!
//! Run once after upgrading to v0.2. Safe to re-run (already-migrated dirs are skipped).
//!
//! What it does for each URL-slug directory in `~/.odoru/articles/`:
//!   1. Assigns a new UUID
//!   2. Renames the directory to `~/.odoru/documents/<uuid>/`
//!   3. Rewrites article.md → document.md frontmatter:
//!      - Adds `id`, `status: ready`, `content_hash` fields
//!      - Removes `synthesized_voices`, `voice_durations`, `published_voice`
//!      - Keeps `url` as `source_url`, `publish`, all metadata
//!   4. Renames `article.txt` → `document.txt`
//!   5. Writes `voices.json` from old `synthesized_voices`/`voice_durations`/`published_voice`
//!   6. Fetches and saves `source.html` for content hash (skipped if unreachable — noted in output)
//!   7. Populates `~/.odoru/index/source_url.json` and `content_hash.json`
//!   8. Patches persisted job files: sets `document_id`, removes stale `article_id`/`article_url`
//!
//! Also moves any already-UUID-keyed dirs still in `articles/` to `documents/`
//! and renames `article.md`/`article.txt` → `document.md`/`document.txt` within them.
//! Job files already containing `document_id` (and no stale `article_id`) are skipped. Safe to re-run.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

fn main() -> Result<()> {
    let home = std::env::var("HOME").context("$HOME not set")?;
    let articles_dir = PathBuf::from(&home).join(".odoru").join("articles");
    let documents_dir = PathBuf::from(&home).join(".odoru").join("documents");
    let index_dir = PathBuf::from(&home).join(".odoru").join("index");
    std::fs::create_dir_all(&index_dir)?;
    std::fs::create_dir_all(&documents_dir)?;

    if !articles_dir.exists() {
        println!("No articles directory found at {} — nothing to migrate.", articles_dir.display());
        // Still run job patching in case articles were already moved.
        let source_url_index = load_index(&index_dir, "source_url.json");
        migrate_jobs(&home, &source_url_index)?;
        return Ok(());
    }

    // Load existing indexes so we don't lose already-migrated entries.
    let mut source_url_index = load_index(&index_dir, "source_url.json");
    let mut content_hash_index = load_index(&index_dir, "content_hash.json");

    let entries: Vec<_> = std::fs::read_dir(&articles_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();

    println!("Found {} article directories.", entries.len());
    let mut migrated = 0;
    let mut skipped = 0;

    for entry in entries {
        let dir = entry.path();
        // If only document.md exists (fully migrated), just move the directory.
        let doc_md_path = dir.join("document.md");
        let md_path = dir.join("article.md");
        if doc_md_path.exists() && !md_path.exists() {
            if let Some(uuid) = std::fs::read_to_string(&doc_md_path).ok()
                .and_then(|s| extract_uuid_id(&s))
            {
                let target = documents_dir.join(&uuid);
                if target.exists() {
                    println!("  SKIP (already in documents/): {uuid}");
                } else {
                    std::fs::rename(&dir, &target)
                        .with_context(|| format!("rename {} → {}", dir.display(), target.display()))?;
                    println!("  MOVED to documents/: {uuid}");
                    migrated += 1;
                }
                skipped += 1;
                continue;
            }
        }

        if !md_path.exists() {
            println!("  SKIP (no article.md or document.md): {}", dir.display());
            skipped += 1;
            continue;
        }

        let src = std::fs::read_to_string(&md_path)
            .with_context(|| format!("read {}", md_path.display()))?;

        // If article.md parses as new format (has UUID id field), just rename + move.
        if let Some(uuid) = extract_uuid_id(&src) {
            let target = documents_dir.join(&uuid);
            if target.exists() {
                rename_files_in_dir(&target);
                println!("  SKIP (already in documents/): {uuid}");
            } else {
                rename_files_in_dir(&dir);
                std::fs::rename(&dir, &target)
                    .with_context(|| format!("rename {} → {}", dir.display(), target.display()))?;
                println!("  MOVED to documents/: {uuid}");
                migrated += 1;
            }
            // Update index from the new-format frontmatter fields.
            if let Ok(idx) = extract_new_format_index(&src) {
                if let Some(url) = idx.0 { source_url_index.insert(url, uuid.clone()); }
                if let Some(hash) = idx.1 { content_hash_index.insert(hash, uuid); }
            }
            skipped += 1;
            continue;
        }

        // Parse as legacy frontmatter.
        let (legacy, body) = match parse_legacy_frontmatter(&src) {
            Ok(p) => p,
            Err(e) => {
                println!("  SKIP (parse error): {} — {e}", dir.display());
                skipped += 1;
                continue;
            }
        };

        // Already UUID-keyed — may still be in articles/ and need moving to documents/.
        if is_uuid(&legacy.id) {
            source_url_index.insert(legacy.url.clone(), legacy.id.clone());
            if let Some(h) = &legacy.content_hash {
                content_hash_index.insert(h.clone(), legacy.id.clone());
            }
            let target = documents_dir.join(&legacy.id);
            if target.exists() {
                // Already in documents/ — just rename files if needed.
                rename_files_in_dir(&target);
                println!("  SKIP (already in documents/): {}", legacy.url);
            } else {
                // Still in articles/ — move it.
                rename_files_in_dir(&dir);
                std::fs::rename(&dir, &target)
                    .with_context(|| format!("rename {} → {}", dir.display(), target.display()))?;
                println!("  MOVED to documents/: {}", legacy.url);
                migrated += 1;
            }
            skipped += 1;
            continue;
        }

        let uuid = uuid::Uuid::new_v4().to_string();
        let new_dir = documents_dir.join(&uuid);

        // Fetch source.html and compute content hash.
        let (source_html, content_hash) = fetch_html_and_hash(&legacy.url);

        // Build voices.json from legacy fields.
        let voices = build_voices_json(&legacy);

        // Write new document.md (atomic: write to temp in old dir, rename dir, done).
        let new_fm = NewFrontmatter {
            id: uuid.clone(),
            status: "ready".to_string(),
            source_url: Some(legacy.url.clone()),
            title: legacy.title.clone(),
            authors: legacy.authors.clone(),
            date: legacy.date.clone(),
            description: legacy.description.clone(),
            cached_at: Some(legacy.cached_at.clone()),
            publish: legacy.publish,
            content_hash: content_hash.clone(),
        };

        let new_yaml = serde_yaml::to_string(&new_fm)
            .context("serialize new frontmatter")?;
        let new_md = format!("---\n{}---\n{}", new_yaml, body);

        // Write document.md into old dir (replacing article.md), rename txt, then move dir.
        let doc_md_path = dir.join("document.md");
        std::fs::write(&doc_md_path, &new_md)
            .with_context(|| format!("write {}", doc_md_path.display()))?;
        // Remove old article.md now that document.md is written.
        let _ = std::fs::remove_file(&md_path);

        // Rename article.txt → document.txt if present.
        let txt_path = dir.join("article.txt");
        if txt_path.exists() {
            std::fs::rename(&txt_path, dir.join("document.txt"))
                .with_context(|| format!("rename article.txt in {}", dir.display()))?;
        }

        // Write voices.json.
        let voices_json = serde_json::to_string_pretty(&voices)?;
        std::fs::write(dir.join("voices.json"), &voices_json)
            .with_context(|| format!("write voices.json in {}", dir.display()))?;

        // Write source.html if we fetched it.
        if let Some(ref html) = source_html {
            std::fs::write(dir.join("source.html"), html)
                .with_context(|| format!("write source.html in {}", dir.display()))?;
        }

        // Move directory to documents/<uuid>/.
        std::fs::rename(&dir, &new_dir)
            .with_context(|| format!("rename {} → {}", dir.display(), new_dir.display()))?;

        // Update indexes.
        source_url_index.insert(legacy.url.clone(), uuid.clone());
        if let Some(ref h) = content_hash {
            content_hash_index.insert(h.clone(), uuid.clone());
        }

        println!("  OK: {} → {uuid}{}", legacy.url,
            if source_html.is_none() { " (source.html skipped — fetch failed)" } else { "" });
        migrated += 1;
    }

    // Write indexes atomically.
    flush_json(&index_dir, "source_url.json", &source_url_index)?;
    flush_json(&index_dir, "content_hash.json", &content_hash_index)?;

    println!("\nDone. Migrated: {migrated}, Skipped: {skipped}");
    println!("Indexes written to {}", index_dir.display());

    // Patch persisted jobs: add document_id where article_url matches the index.
    migrate_jobs(&home, &source_url_index)?;

    Ok(())
}

fn migrate_jobs(home: &str, source_url_index: &HashMap<String, String>) -> Result<()> {
    let jobs_dir = PathBuf::from(home).join(".odoru").join("jobs");
    if !jobs_dir.exists() {
        return Ok(());
    }

    let entries: Vec<_> = std::fs::read_dir(&jobs_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
        .collect();

    println!("\nPatching {} job file(s)…", entries.len());
    let mut patched = 0;
    let mut job_skipped = 0;

    for entry in entries {
        let path = entry.path();
        let src = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => { println!("  SKIP (unreadable): {} — {e}", path.display()); job_skipped += 1; continue; }
        };

        let mut job: serde_json::Value = match serde_json::from_str(&src) {
            Ok(v) => v,
            Err(e) => { println!("  SKIP (parse error): {} — {e}", path.display()); job_skipped += 1; continue; }
        };

        // Already has document_id and no stale article_id — skip.
        let has_document_id = job.get("document_id").and_then(|v| v.as_str()).is_some();
        let has_article_id = job.get("article_id").and_then(|v| v.as_str()).is_some();
        if has_document_id && !has_article_id {
            job_skipped += 1;
            continue;
        }

        // Migrate from article_id → document_id if present, else use article_url index.
        let uuid = if let Some(id) = job.get("article_id").and_then(|v| v.as_str()) {
            id.to_string()
        } else {
            let article_url = match job.get("article_url").and_then(|v| v.as_str()) {
                Some(u) => u.to_string(),
                None => { job_skipped += 1; continue; }
            };
            match source_url_index.get(&article_url) {
                Some(id) => id.clone(),
                None => {
                    println!("  SKIP (no index entry for url): {article_url}");
                    job_skipped += 1;
                    continue;
                }
            }
        };

        job["document_id"] = serde_json::Value::String(uuid.clone());
        // Remove stale article_id field if present.
        if let Some(obj) = job.as_object_mut() {
            obj.remove("article_id");
            obj.remove("article_url");
        }

        let updated = serde_json::to_string_pretty(&job)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &updated)?;
        std::fs::rename(&tmp, &path)?;

        println!("  OK job: → {uuid}");
        patched += 1;
    }

    println!("Jobs patched: {patched}, Skipped: {job_skipped}");
    Ok(())
}

// ---------------------------------------------------------------------------
// Legacy frontmatter (v0.1 schema)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct LegacyFrontmatter {
    /// In v0.1 this was the URL slug used as a directory name; not a UUID.
    /// We use it to detect already-migrated entries.
    #[serde(default)]
    id: String,
    url: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    authors: Vec<String>,
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    description: Option<String>,
    cached_at: String,
    #[serde(default)]
    synthesized_voices: Vec<String>,
    #[serde(default)]
    voice_durations: HashMap<String, f64>,
    #[serde(default)]
    publish: bool,
    #[serde(default)]
    published_voice: Option<String>,
    /// Present only if already migrated.
    #[serde(default)]
    content_hash: Option<String>,
}

fn parse_legacy_frontmatter(src: &str) -> Result<(LegacyFrontmatter, &str)> {
    let src = src.trim_start_matches('\n');
    if !src.starts_with("---") {
        anyhow::bail!("missing opening '---'");
    }
    let after = &src[3..];
    let close = after.find("\n---").context("missing closing '---'")?;
    let yaml = &after[..close];
    let body = &after[close + 4..];
    let body = body.strip_prefix('\n').unwrap_or(body);
    let fm: LegacyFrontmatter = serde_yaml::from_str(yaml).context("parse YAML")?;
    Ok((fm, body))
}

fn is_uuid(s: &str) -> bool {
    // UUID v4 format: 8-4-4-4-12 hex chars
    uuid::Uuid::parse_str(s).is_ok()
}

// ---------------------------------------------------------------------------
// New frontmatter (v0.2 schema)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct NewFrontmatter {
    id: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    authors: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cached_at: Option<String>,
    publish: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_hash: Option<String>,
}

// ---------------------------------------------------------------------------
// voices.json from legacy fields
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct VoiceEntry {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration: Option<f64>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    published: bool,
}

fn build_voices_json(legacy: &LegacyFrontmatter) -> HashMap<String, VoiceEntry> {
    let mut voices = HashMap::new();
    for voice_id in &legacy.synthesized_voices {
        let duration = legacy.voice_durations.get(voice_id).copied();
        let published = legacy.published_voice.as_deref() == Some(voice_id);
        voices.insert(voice_id.clone(), VoiceEntry {
            status: "ready".to_string(),
            duration,
            published,
        });
    }
    voices
}

// ---------------------------------------------------------------------------
// Fetch source.html
// ---------------------------------------------------------------------------

fn fetch_html_and_hash(url: &str) -> (Option<String>, Option<String>) {
    use sha2::{Digest, Sha256};

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36")
        .redirect(reqwest::redirect::Policy::limited(5))
        .build();

    let client = match client {
        Ok(c) => c,
        Err(_) => return (None, None),
    };

    match client.get(url).send().and_then(|r| r.text()) {
        Ok(html) => {
            let mut h = Sha256::new();
            h.update(html.as_bytes());
            let hash = format!("{:x}", h.finalize());
            (Some(html), Some(hash))
        }
        Err(e) => {
            eprintln!("    Warning: could not fetch {url}: {e}");
            (None, None)
        }
    }
}

// ---------------------------------------------------------------------------
// New-format detection helpers
// ---------------------------------------------------------------------------

/// If the frontmatter contains an `id` field that is a valid UUID, return it.
/// Used to detect already-migrated article.md files without full parsing.
fn extract_uuid_id(src: &str) -> Option<String> {
    let src = src.trim_start_matches('\n');
    if !src.starts_with("---") { return None; }
    let after = &src[3..];
    let close = after.find("\n---")?;
    let yaml = &after[..close];
    // Quick scan for `id: <uuid>` line — avoid full deserialization.
    for line in yaml.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("id:") {
            let candidate = rest.trim().trim_matches('\'').trim_matches('"');
            if is_uuid(candidate) {
                return Some(candidate.to_string());
            }
        }
    }
    None
}

/// Extract (source_url, content_hash) from new-format frontmatter for index population.
fn extract_new_format_index(src: &str) -> anyhow::Result<(Option<String>, Option<String>)> {
    #[derive(serde::Deserialize)]
    struct Minimal {
        #[serde(default)]
        source_url: Option<String>,
        #[serde(default)]
        content_hash: Option<String>,
    }
    let src = src.trim_start_matches('\n');
    let after = &src[3..];
    let close = after.find("\n---").ok_or_else(|| anyhow::anyhow!("no close"))?;
    let yaml = &after[..close];
    let m: Minimal = serde_yaml::from_str(yaml)?;
    Ok((m.source_url, m.content_hash))
}

// ---------------------------------------------------------------------------
// File rename helpers
// ---------------------------------------------------------------------------

/// Rename article.md → document.md and article.txt → document.txt within a dir.
/// No-ops if files don't exist or already renamed.
fn rename_files_in_dir(dir: &PathBuf) {
    let article_md = dir.join("article.md");
    let document_md = dir.join("document.md");
    if article_md.exists() && !document_md.exists() {
        let _ = std::fs::rename(&article_md, &document_md);
    }
    let article_txt = dir.join("article.txt");
    let document_txt = dir.join("document.txt");
    if article_txt.exists() && !document_txt.exists() {
        let _ = std::fs::rename(&article_txt, &document_txt);
    }
}

// ---------------------------------------------------------------------------
// Index helpers
// ---------------------------------------------------------------------------

fn load_index(dir: &PathBuf, filename: &str) -> HashMap<String, String> {
    std::fs::read_to_string(dir.join(filename))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn flush_json(dir: &PathBuf, filename: &str, map: &HashMap<String, String>) -> Result<()> {
    let path = dir.join(filename);
    let tmp = dir.join(format!("{filename}.tmp"));
    let json = serde_json::to_string_pretty(map)?;
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}
