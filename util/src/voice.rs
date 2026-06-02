//! Voice definition loading from a `voices/<name>/` directory.
//!
//! Each voice lives in its own directory containing:
//!   - `voice.md`  — YAML frontmatter with synthesis params, body is
//!                   a human-readable description for the UI
//!   - `ref.wav`   — mono 24kHz reference audio clip
//!
//! # Example
//!
//! ```no_run
//! use std::path::Path;
//! use util::voice::VoiceDef;
//!
//! let def = VoiceDef::load(Path::new("voices/sarah")).unwrap();
//! assert_eq!(def.name, "sarah");
//! ```

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::frontmatter;

// ---------------------------------------------------------------------------
// Frontmatter schema for voice.md
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct VoiceFrontmatter {
    transcript: String,
    #[serde(default = "default_speed")]
    speed: f32,
    #[serde(default = "default_cfg_strength")]
    cfg_strength: f32,
}

fn default_speed() -> f32 { 0.85 }
fn default_cfg_strength() -> f32 { 2.0 }

// ---------------------------------------------------------------------------
// Public type
// ---------------------------------------------------------------------------

/// A loaded voice definition, ready to pass to `tts::Voice::F5Tts`.
#[derive(Debug, Clone)]
pub struct VoiceDef {
    /// Voice name — taken from the directory name, lowercased.
    pub name: String,
    /// Path to the reference WAV file (`<dir>/ref.wav`).
    pub voice_ref: PathBuf,
    /// Exact transcript of the reference clip.
    pub ref_text: String,
    /// Speech speed multiplier (default 0.85).
    pub speed: f32,
    /// Classifier-free guidance strength (default 2.0).
    pub cfg_strength: f32,
    /// Human-readable description from the body of `voice.md`.
    pub description: String,
}

impl VoiceDef {
    /// Load a voice from a directory containing `voice.md` and `ref.wav`.
    ///
    /// The voice name is derived from the directory's file name.
    pub fn load(dir: &Path) -> Result<Self> {
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.to_lowercase())
            .with_context(|| format!("voice directory has no name: {}", dir.display()))?;

        let md_path = dir.join("voice.md");
        let src = std::fs::read_to_string(&md_path)
            .with_context(|| format!("failed to read {}", md_path.display()))?;

        let (fm, body) = frontmatter::parse::<VoiceFrontmatter>(&src)
            .with_context(|| format!("failed to parse {}", md_path.display()))?;

        let voice_ref = dir.join("ref.wav");
        if !voice_ref.exists() {
            anyhow::bail!("ref.wav not found in {}", dir.display());
        }

        Ok(Self {
            name,
            voice_ref,
            ref_text: fm.transcript,
            speed: fm.speed,
            cfg_strength: fm.cfg_strength,
            description: body.trim().to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write_voice(dir: &Path, md: &str) {
        fs::write(dir.join("voice.md"), md).unwrap();
        fs::write(dir.join("ref.wav"), b"").unwrap(); // stub
    }

    #[test]
    fn load_full_voice() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join("sarah");
        fs::create_dir(&dir).unwrap();
        write_voice(&dir, "---\ntranscript: \"Hello world.\"\nspeed: 0.9\ncfg_strength: 1.5\n---\nSarah's voice.");
        let v = VoiceDef::load(&dir).unwrap();
        assert_eq!(v.name, "sarah");
        assert_eq!(v.ref_text, "Hello world.");
        assert!((v.speed - 0.9).abs() < 0.001);
        assert!((v.cfg_strength - 1.5).abs() < 0.001);
        assert_eq!(v.description, "Sarah's voice.");
    }

    #[test]
    fn load_uses_defaults_for_optional_fields() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join("bob");
        fs::create_dir(&dir).unwrap();
        write_voice(&dir, "---\ntranscript: \"Hi there.\"\n---\n");
        let v = VoiceDef::load(&dir).unwrap();
        assert!((v.speed - 0.85).abs() < 0.001);
        assert!((v.cfg_strength - 2.0).abs() < 0.001);
    }

    #[test]
    fn load_missing_ref_wav_errors() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join("ghost");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("voice.md"), "---\ntranscript: \"Hi.\"\n---\n").unwrap();
        // no ref.wav
        assert!(VoiceDef::load(&dir).is_err());
    }

    #[test]
    fn load_missing_voice_md_errors() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join("nobody");
        fs::create_dir(&dir).unwrap();
        assert!(VoiceDef::load(&dir).is_err());
    }
}
