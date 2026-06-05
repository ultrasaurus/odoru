//! In-memory document indexes with disk persistence.
//!
//! Two indexes live in `~/.odoru/index/`:
//!   `source_url.json`   — url → uuid
//!   `content_hash.json` — sha256(source.html) → uuid
//!
//! Both are loaded into memory at startup. Reads need no lock. Writes
//! acquire a `RwLock`, update in-memory state, then flush to disk
//! (write-to-temp-then-rename for crash safety).
//!
//! On flush failure: log loudly, write `.rebuild-needed` sentinel.
//! On startup with sentinel present: rebuild from article frontmatter.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::documents;

// ---------------------------------------------------------------------------
// Sentinel
// ---------------------------------------------------------------------------

const SENTINEL: &str = ".rebuild-needed";

// ---------------------------------------------------------------------------
// DocumentIndex
// ---------------------------------------------------------------------------

pub struct DocumentIndex {
    /// url → uuid
    source_url: RwLock<HashMap<String, String>>,
    /// sha256(source.html) → uuid
    content_hash: RwLock<HashMap<String, String>>,
    index_dir: PathBuf,
}

impl DocumentIndex {
    /// Load indexes from `~/.odoru/index/`, rebuilding if the sentinel is present.
    pub async fn load() -> Result<Self> {
        let home = std::env::var("HOME").context("$HOME not set")?;
        let index_dir = PathBuf::from(home).join(".odoru").join("index");
        std::fs::create_dir_all(&index_dir)
            .with_context(|| format!("failed to create index dir {}", index_dir.display()))?;

        let idx = Self {
            source_url: RwLock::new(HashMap::new()),
            content_hash: RwLock::new(HashMap::new()),
            index_dir,
        };

        if idx.index_dir.join(SENTINEL).exists() {
            info!("[index] .rebuild-needed sentinel found — rebuilding indexes");
            idx.rebuild_from_disk().await?;
            if let Err(e) = std::fs::remove_file(idx.index_dir.join(SENTINEL)) {
                warn!("[index] failed to remove sentinel: {e}");
            }
            info!("[index] index rebuild complete");
        } else {
            idx.load_from_disk().await?;
        }

        Ok(idx)
    }

    // -----------------------------------------------------------------------
    // Reads (no lock needed — caller reads from RwLock)
    // -----------------------------------------------------------------------

    pub async fn get_by_source_url(&self, url: &str) -> Option<String> {
        self.source_url.read().await.get(url).cloned()
    }

    pub async fn get_by_content_hash(&self, hash: &str) -> Option<String> {
        self.content_hash.read().await.get(hash).cloned()
    }

    // -----------------------------------------------------------------------
    // Writes
    // -----------------------------------------------------------------------

    /// Insert a new document into both indexes. Flushes to disk.
    pub async fn insert(
        &self,
        uuid: &str,
        source_url: Option<&str>,
        content_hash: Option<&str>,
    ) {
        {
            let mut su = self.source_url.write().await;
            let mut ch = self.content_hash.write().await;
            if let Some(url) = source_url {
                su.insert(url.to_string(), uuid.to_string());
            }
            if let Some(hash) = content_hash {
                ch.insert(hash.to_string(), uuid.to_string());
            }
        }
        self.flush().await;
    }

    // -----------------------------------------------------------------------
    // Disk flush
    // -----------------------------------------------------------------------

    async fn flush(&self) {
        let su_snapshot = self.source_url.read().await.clone();
        let ch_snapshot = self.content_hash.read().await.clone();

        if let Err(e) = flush_json(&self.index_dir, "source_url.json", &su_snapshot) {
            error!("[index] failed to flush source_url.json: {e}");
            self.write_sentinel();
        }
        if let Err(e) = flush_json(&self.index_dir, "content_hash.json", &ch_snapshot) {
            error!("[index] failed to flush content_hash.json: {e}");
            self.write_sentinel();
        }
    }

    fn write_sentinel(&self) {
        let path = self.index_dir.join(SENTINEL);
        if let Err(e) = std::fs::write(&path, "") {
            error!("[index] CRITICAL: failed to write rebuild sentinel {}: {e}", path.display());
        }
    }

    // -----------------------------------------------------------------------
    // Load / rebuild
    // -----------------------------------------------------------------------

    async fn load_from_disk(&self) -> Result<()> {
        *self.source_url.write().await =
            load_json(&self.index_dir, "source_url.json").unwrap_or_default();
        *self.content_hash.write().await =
            load_json(&self.index_dir, "content_hash.json").unwrap_or_default();
        Ok(())
    }

    async fn rebuild_from_disk(&self) -> Result<()> {
        let docs = documents::list_all()?;
        let mut su: HashMap<String, String> = HashMap::new();
        let mut ch: HashMap<String, String> = HashMap::new();

        for doc in docs {
            if let Some(url) = &doc.source_url {
                su.insert(url.clone(), doc.id.clone());
            }
            if let Some(hash) = &doc.content_hash {
                ch.insert(hash.clone(), doc.id.clone());
            }
        }

        *self.source_url.write().await = su;
        *self.content_hash.write().await = ch;
        self.flush().await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Content hash
// ---------------------------------------------------------------------------

/// Compute SHA-256 of raw HTML bytes, returned as a hex string.
pub fn html_content_hash(html: &str) -> String {
    let mut h = Sha256::new();
    h.update(html.as_bytes());
    format!("{:x}", h.finalize())
}

// ---------------------------------------------------------------------------
// Disk helpers
// ---------------------------------------------------------------------------

fn flush_json(dir: &PathBuf, filename: &str, map: &HashMap<String, String>) -> Result<()> {
    let path = dir.join(filename);
    let tmp = dir.join(format!("{filename}.tmp"));
    let json = serde_json::to_string_pretty(map).context("serialize")?;
    std::fs::write(&tmp, &json)
        .with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("rename {} → {}", tmp.display(), path.display()))?;
    Ok(())
}

fn load_json(dir: &PathBuf, filename: &str) -> Option<HashMap<String, String>> {
    let path = dir.join(filename);
    let src = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&src).ok()
}
