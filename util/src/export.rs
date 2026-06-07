//! Shared data types for static SPA export.

use serde::{Deserialize, Serialize};

/// One sentence in an exported transcript.
///
/// `start` and `end` are in seconds, accumulated from per-sentence durations.
/// Both are `0.0` in Stage 2 (text-only export); populated in Stage 3 once
/// audio cache lookup is wired up.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportTranscriptEntry {
    pub index: usize,
    pub text: String,
    pub start: f64,
    pub end: f64,
    pub paragraph_end: bool,
}

/// One entry in the export manifest — shown in the document sidebar.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub slug: String,
    pub title: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub authors: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}
