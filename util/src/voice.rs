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
use tracing::warn;

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

    /// Load all voices from a directory, sorted by name.
    ///
    /// Subdirectories that don't contain a valid `voice.md` + `ref.wav` are
    /// silently skipped. Non-directory entries (files, symlinks) are ignored.
    pub fn load_all(dir: &Path) -> Result<Vec<Self>> {
        let mut voices = Vec::new();

        let entries = std::fs::read_dir(dir)
            .with_context(|| format!("failed to read voices directory: {}", dir.display()))?;

        for entry in entries {
            let entry = entry.with_context(|| format!("error reading entry in {}", dir.display()))?;
            let path = entry.path();
            if !path.is_dir() { continue; }
            match Self::load(&path) {
                Ok(v) => voices.push(v),
                Err(e) => warn!("Skipping voice {}: {e}", path.display()),
            }
        }

        voices.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(voices)
    }
}

/// Resolve a voices directory by subpath.
///
/// Resolution order:
///   1. `$VOICES_DIR` environment variable (used as-is; `subpath` is ignored)
///   2. Next to the installed binary (`<exe_dir>/<subpath>`)
///   3. Compile-time path (`CARGO_MANIFEST_DIR/../<subpath>`)
pub fn voices_dir_for(subpath: &str) -> Result<PathBuf> {
    // 1. Env var (full path override — subpath not appended)
    if let Ok(dir) = std::env::var("VOICES_DIR") {
        let p = PathBuf::from(dir);
        if p.is_dir() { return Ok(p); }
        anyhow::bail!("VOICES_DIR is set but '{}' is not a directory", p.display());
    }

    // 2. Next to the binary
    if let Ok(exe) = std::env::current_exe() {
        let p = exe.parent().unwrap_or(Path::new(".")).join(subpath);
        if p.is_dir() { return Ok(p); }
    }

    // 3. Compile-time path
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join(subpath);
    if p.is_dir() { return Ok(p); }

    anyhow::bail!(
        "voices directory not found at '{subpath}'. Set $VOICES_DIR or place a {subpath}/ directory next to the binary."
    )
}

/// Resolve the voices directory (F5-TTS voices).
///
/// Resolution order:
///   1. `$VOICES_DIR` environment variable
///   2. Next to the installed binary (`<exe_dir>/voices`)
///   3. Compile-time path (`CARGO_MANIFEST_DIR/../voices`)
pub fn voices_dir() -> Result<PathBuf> {
    voices_dir_for("voices")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex;
    use tempfile::tempdir;

    // Serialize tests that mutate VOICES_DIR env var
    static ENV_LOCK: Mutex<()> = Mutex::new(());

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
    fn load_all_returns_sorted_voices() {
        let tmp = tempdir().unwrap();
        for name in &["zebra", "alice", "bob"] {
            let dir = tmp.path().join(name);
            fs::create_dir(&dir).unwrap();
            write_voice(&dir, "---\ntranscript: \"Hi.\"\n---\n");
        }
        // Add a non-directory file — should be ignored
        fs::write(tmp.path().join("not-a-voice.txt"), "").unwrap();

        let voices = VoiceDef::load_all(tmp.path()).unwrap();
        assert_eq!(voices.len(), 3);
        assert_eq!(voices[0].name, "alice");
        assert_eq!(voices[1].name, "bob");
        assert_eq!(voices[2].name, "zebra");
    }

    #[test]
    fn load_all_skips_invalid_voice_dirs() {
        let tmp = tempdir().unwrap();
        // Valid voice
        let good = tmp.path().join("good");
        fs::create_dir(&good).unwrap();
        write_voice(&good, "---\ntranscript: \"Hi.\"\n---\n");
        // Invalid — missing ref.wav (write_voice stubs it, so just make an empty dir)
        let bad = tmp.path().join("bad");
        fs::create_dir(&bad).unwrap();
        // no voice.md, no ref.wav

        let voices = VoiceDef::load_all(tmp.path()).unwrap();
        assert_eq!(voices.len(), 1);
        assert_eq!(voices[0].name, "good");
    }

    #[test]
    fn voices_dir_uses_env_var() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempdir().unwrap();
        std::env::set_var("VOICES_DIR", tmp.path());
        let result = voices_dir();
        std::env::remove_var("VOICES_DIR");
        assert_eq!(result.unwrap(), tmp.path());
    }

    #[test]
    fn voices_dir_env_var_not_dir_errors() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("VOICES_DIR", "/nonexistent/path");
        let result = voices_dir();
        std::env::remove_var("VOICES_DIR");
        assert!(result.is_err());
    }

    #[test]
    fn voices_dir_for_uses_subpath_next_to_binary() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // No VOICES_DIR set; rely on the subpath being absent next to the binary
        // and the compile-time path. We can't easily test the binary-adjacent
        // case in unit tests, but we verify that passing a subpath that doesn't
        // exist anywhere returns an error.
        std::env::remove_var("VOICES_DIR");
        assert!(voices_dir_for("nonexistent/subpath/xyz").is_err());
    }

    #[test]
    fn voices_dir_for_env_var_ignores_subpath() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempdir().unwrap();
        std::env::set_var("VOICES_DIR", tmp.path());
        // subpath is ignored when env var is set
        let result = voices_dir_for("some/other/path");
        std::env::remove_var("VOICES_DIR");
        assert_eq!(result.unwrap(), tmp.path());
    }

    #[test]
    fn load_missing_voice_md_errors() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join("nobody");
        fs::create_dir(&dir).unwrap();
        assert!(VoiceDef::load(&dir).is_err());
    }
}
