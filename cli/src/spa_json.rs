//! Serializable types for the `dl spa --json` output:
//!   `index.json`          — document manifest
//!   `<slug>.json`         — per-document sentences with word timing

use serde::Serialize;

const SCHEMA_VERSION: &str = "0.1";

#[derive(Serialize)]
pub struct SpaIndex {
    pub schema_version: &'static str,
    pub documents: Vec<SpaIndexEntry>,
}

impl SpaIndex {
    pub fn new(documents: Vec<SpaIndexEntry>) -> Self {
        Self { schema_version: SCHEMA_VERSION, documents }
    }
}

#[derive(Serialize)]
pub struct SpaIndexEntry {
    pub slug: String,
    pub title: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    pub has_audio: bool,
}

#[derive(Serialize)]
pub struct SpaDoc {
    pub schema_version: &'static str,
    pub doc_id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice_id: Option<String>,
    pub source_sha256: String,
    pub sentences: Vec<SpaSentence>,
}

impl SpaDoc {
    pub fn new(
        doc_id: String,
        title: String,
        authors: Vec<String>,
        date: Option<String>,
        description: Option<String>,
        source_url: Option<String>,
        voice_id: Option<String>,
        source_sha256: String,
        sentences: Vec<SpaSentence>,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            doc_id,
            title,
            authors,
            date,
            description,
            source_url,
            voice_id,
            source_sha256,
            sentences,
        }
    }
}

#[derive(Serialize)]
pub struct SpaSentence {
    pub index: usize,
    pub text: String,
    pub markdown: String,
    pub duration: f64,
    pub paragraph_end: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub words: Option<Vec<SpaWord>>,
}

#[derive(Serialize)]
pub struct SpaWord {
    pub word: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<f64>,
}
