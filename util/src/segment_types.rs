//! `<basedir>/<name>.segments.json` — the sidecar vibe writes alongside its
//! per-segment output. See `vibe/dev/odoru-import-prep.md` for the full
//! design and `dev/tts-backends/vibe-import.md` for how Odoru's CLI consumes
//! it.
//!
//! Vibe writes the `sentences`/`files.original` parts at split time;
//! `synthesize` fills in `files.normalized`/`audio`/`transcript`/`report` and
//! `voice_id` per segment as each one is rendered. Odoru's `dl import vibe`
//! reads the fully-populated sidecar.
//!
//! Lives in `util` (not `vibe`) so both vibe (writer) and the `dl` CLI
//! (reader) depend on one schema instead of two copies drifting apart.

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Sidecar {
    pub schema_version: String,
    pub source_document: String,
    pub source_sha256: String,
    pub voice_id: Option<String>,
    pub segments: Vec<SidecarSegment>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct SidecarSegment {
    pub index: u32,
    pub sentences: Vec<SidecarSentence>,
    pub files: SidecarFiles,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct SidecarSentence {
    pub text: String,
    pub paragraph_end: bool,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
pub struct SidecarFiles {
    pub original: Option<String>,
    pub normalized: Option<String>,
    pub audio: Option<String>,
    pub transcript: Option<String>,
    pub report: Option<String>,
}
