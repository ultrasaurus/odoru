//! Voice definitions and upload helpers for the vibe CLI.
//!
//! Each voice lives in `vibe/voices/<name>/`, containing:
//!   - `voice.md` — YAML frontmatter with synthesis params, body is a
//!     human-readable description
//!   - `ref.wav` — reference audio clip uploaded to the service before
//!     synthesizing with that voice

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

// ---------------------------------------------------------------------------
// Frontmatter schema for voice.md
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct VibeFrontmatter {
    transcript: String,
    gender: String,
    #[serde(default)]
    cfg_scale: Option<f64>,
    #[serde(default)]
    seed: Option<u64>,
    #[serde(default)]
    speed: Option<f64>,
    #[serde(default)]
    temp: Option<f64>,
    #[serde(default = "default_language")]
    language: String,
}

fn default_language() -> String { "en".to_string() }

// ---------------------------------------------------------------------------
// Public type
// ---------------------------------------------------------------------------

/// A loaded vibe voice definition, ready to pass to the vibe-service API.
#[allow(dead_code)] // transcript/language/description mirror voice.md fully; not all are consumed yet
#[derive(Debug, Clone)]
pub struct VibeVoiceDef {
    /// Voice name — taken from the directory name.
    pub name: String,
    /// Path to the reference wav file (`<dir>/ref.wav`).
    pub wav_path: PathBuf,
    /// Exact transcript of the reference clip.
    pub transcript: String,
    /// Voice descriptor matching VibeVoice's filename convention (e.g. "man", "woman").
    pub gender: String,
    pub cfg_scale: Option<f64>,
    pub seed: Option<u64>,
    pub speed: Option<f64>,
    pub temp: Option<f64>,
    pub language: String,
    /// Human-readable description from the body of `voice.md`.
    pub description: String,
}

impl VibeVoiceDef {
    /// Load a voice from a directory containing `voice.md` and `ref.wav`.
    pub fn load(dir: &Path) -> Result<Self> {
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .with_context(|| format!("voice directory has no name: {}", dir.display()))?
            .to_string();

        let md_path = dir.join("voice.md");
        let src = std::fs::read_to_string(&md_path)
            .with_context(|| format!("failed to read {}", md_path.display()))?;

        let (fm, body) = util::frontmatter::parse::<VibeFrontmatter>(&src)
            .with_context(|| format!("failed to parse {}", md_path.display()))?;

        let wav_path = dir.join("ref.wav");
        if !wav_path.exists() {
            anyhow::bail!("ref.wav not found in {}", dir.display());
        }

        Ok(Self {
            name,
            wav_path,
            transcript: fm.transcript,
            gender: fm.gender,
            cfg_scale: fm.cfg_scale,
            seed: fm.seed,
            speed: fm.speed,
            temp: fm.temp,
            language: fm.language,
            description: body.trim().to_string(),
        })
    }

    /// Load a voice by name from the resolved vibe voices directory.
    pub fn load_named(name: &str) -> Result<Self> {
        let dir = vibe_voices_dir()?.join(name);
        Self::load(&dir)
    }
}

/// Resolve the vibe voices directory (`vibe/voices/`), distinct from the
/// F5-TTS `voices/` directory used elsewhere in the workspace.
pub fn vibe_voices_dir() -> Result<PathBuf> {
    util::voice::voices_dir_for("vibe/voices")
}

/// POST a reference wav to the vibe-service `/voices/<name>/<gender>` endpoint.
///
/// `base_url` should already be the resolved, health-checked service URL.
pub async fn upload_wav(
    http: &reqwest::Client,
    base_url: &str,
    name: &str,
    gender: &str,
    bytes: Vec<u8>,
    secret: &Option<String>,
) -> Result<()> {
    let mut req = http
        .post(format!("{base_url}/voices/{name}/{gender}"))
        .body(bytes);
    if let Some(s) = secret {
        req = req.bearer_auth(s);
    }
    let resp = req.send().await.context("POST /voices")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("upload failed: HTTP {status} {body}");
    }
    Ok(())
}

#[cfg(test)]
mod voice_def_tests {
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
        let dir = tmp.path().join("Andy");
        fs::create_dir(&dir).unwrap();
        write_voice(
            &dir,
            "---\ntranscript: \"Hello.\"\ngender: man\ncfg_scale: 1.3\nseed: 993445\nlanguage: en\n---\nAndy's voice.",
        );
        let v = VibeVoiceDef::load(&dir).unwrap();
        assert_eq!(v.name, "Andy");
        assert_eq!(v.gender, "man");
        assert_eq!(v.transcript, "Hello.");
        assert_eq!(v.cfg_scale, Some(1.3));
        assert_eq!(v.seed, Some(993445));
        assert_eq!(v.language, "en");
        assert_eq!(v.description, "Andy's voice.");
    }

    #[test]
    fn load_uses_defaults_for_optional_fields() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join("bob");
        fs::create_dir(&dir).unwrap();
        write_voice(&dir, "---\ntranscript: \"Hi.\"\ngender: man\n---\n");
        let v = VibeVoiceDef::load(&dir).unwrap();
        assert_eq!(v.cfg_scale, None);
        assert_eq!(v.seed, None);
        assert_eq!(v.speed, None);
        assert_eq!(v.temp, None);
        assert_eq!(v.language, "en");
    }

    #[test]
    fn load_missing_ref_wav_errors() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join("ghost");
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("voice.md"), "---\ntranscript: \"Hi.\"\ngender: man\n---\n").unwrap();
        assert!(VibeVoiceDef::load(&dir).is_err());
    }

    #[test]
    fn load_missing_gender_errors() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join("nogender");
        fs::create_dir(&dir).unwrap();
        write_voice(&dir, "---\ntranscript: \"Hi.\"\n---\n");
        assert!(VibeVoiceDef::load(&dir).is_err());
    }

    #[test]
    fn load_named_reads_from_vibe_voices_dir() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join("Sarah");
        fs::create_dir(&dir).unwrap();
        write_voice(&dir, "---\ntranscript: \"Hi.\"\ngender: woman\n---\n");

        std::env::set_var("VOICES_DIR", tmp.path());
        let v = VibeVoiceDef::load_named("Sarah");
        std::env::remove_var("VOICES_DIR");

        assert_eq!(v.unwrap().gender, "woman");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::{method, path}};

    #[tokio::test]
    async fn upload_wav_posts_to_correct_path() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/voices/Andy/man"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let http = reqwest::Client::new();
        upload_wav(&http, &server.uri(), "Andy", "man", b"fakewav".to_vec(), &None)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn upload_wav_returns_error_on_failure() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/voices/Andy/man"))
            .respond_with(ResponseTemplate::new(500).set_body_string("oops"))
            .mount(&server)
            .await;

        let http = reqwest::Client::new();
        let err = upload_wav(&http, &server.uri(), "Andy", "man", b"fakewav".to_vec(), &None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("HTTP 500"));
    }
}
