//! Disk cache for synthesized audio segments.
//!
//! Cache directory: `~/.odoru/audio/`
//!
//! Each entry is two files:
//!   `<hash>.mp3`  — MP3-encoded audio
//!   `<hash>.json` — metadata (text, duration)
//!
//! Cache key: SHA-256(normalized_text + "|" + voice_cache_key)
//! This means changing voice params (speed, cfg_strength) busts the cache.

use tracing::error;
use std::path::PathBuf;

use mp3lame_encoder::{Builder, FlushNoGap, MonoPcm};
use sha2::{Digest, Sha256};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Metadata sidecar
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct Meta {
    text: String,
    duration: f64,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Compute the cache key for a sentence + voice combination.
pub fn cache_key(text: &str, voice_cache_key: &str) -> String {
    let mut h = Sha256::new();
    h.update(text.as_bytes());
    h.update(b"|");
    h.update(voice_cache_key.as_bytes());
    format!("{:x}", h.finalize())
}

/// Encode f32 mono PCM samples to MP3 bytes using LAME.
pub fn encode_mp3(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let mut builder = Builder::new().expect("lame init");
    builder.set_num_channels(1).expect("set channels");
    builder.set_sample_rate(sample_rate).expect("set sample rate");
    builder.set_brate(mp3lame_encoder::Bitrate::Kbps192).expect("set bitrate");
    builder.set_quality(mp3lame_encoder::Quality::NearBest).expect("set quality");
    let mut encoder = builder.build().expect("lame build");

    // LAME requires at least (1.25 * num_samples + 7200) bytes of output buffer.
    let capacity = (samples.len() * 5 / 4) + 7200;
    let mut mp3 = Vec::with_capacity(capacity);
    encoder.encode_to_vec(MonoPcm(samples), &mut mp3).expect("encode");
    encoder.flush_to_vec::<FlushNoGap>(&mut mp3).expect("flush");
    mp3
}

/// Look up a cached segment. Returns `(mp3_bytes, duration)` on hit.
pub fn lookup(key: &str) -> Option<(Vec<u8>, f64)> {
    lookup_in(&cache_dir()?, key)
}

fn lookup_in(dir: &PathBuf, key: &str) -> Option<(Vec<u8>, f64)> {
    let mp3_path = dir.join(format!("{key}.mp3"));
    let json_path = dir.join(format!("{key}.json"));

    if !mp3_path.exists() || !json_path.exists() {
        return None;
    }

    let meta: Meta = {
        let s = std::fs::read_to_string(&json_path).ok()?;
        serde_json::from_str(&s).ok()?
    };

    let mp3_bytes = std::fs::read(&mp3_path).ok()?;
    Some((mp3_bytes, meta.duration))
}

/// Check whether a cache entry exists without reading the audio data.
pub fn exists(key: &str) -> bool {
    let Some(dir) = cache_dir() else { return false; };
    dir.join(format!("{key}.mp3")).exists() && dir.join(format!("{key}.json")).exists()
}

/// Write a synthesized segment to the cache.
pub fn store(key: &str, text: &str, mp3_bytes: &[u8], duration: f64) {
    let Some(dir) = cache_dir() else { return; };
    store_in(&dir, key, text, mp3_bytes, duration);
}

fn store_in(dir: &PathBuf, key: &str, text: &str, mp3_bytes: &[u8], duration: f64) {
    if let Err(e) = std::fs::create_dir_all(dir) {
        error!("audio cache: failed to create dir: {e}");
        return;
    }

    let mp3_path = dir.join(format!("{key}.mp3"));
    if let Err(e) = std::fs::write(&mp3_path, mp3_bytes) {
        error!("audio cache: failed to write mp3: {e}");
        return;
    }

    let json_path = dir.join(format!("{key}.json"));
    let meta = Meta { text: text.to_string(), duration };
    if let Ok(json) = serde_json::to_string(&meta) {
        if let Err(e) = std::fs::write(&json_path, json) {
            error!("audio cache: failed to write metadata: {e}");
        }
    }
}

/// Return the audio cache directory, or `None` if `$HOME` is not set.
pub fn cache_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".odoru").join("audio"))
}

#[cfg(test)]
fn cache_dir_at(base: &PathBuf) -> PathBuf {
    base.join("audio")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sine_samples(n: usize, sample_rate: u32) -> Vec<f32> {
        (0..n)
            .map(|i| 0.3 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / sample_rate as f32).sin())
            .collect()
    }

    #[test]
    fn roundtrip_store_and_lookup() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = cache_dir_at(&tmp.path().to_path_buf());

        let samples = sine_samples(2400, 24_000);
        let mp3 = encode_mp3(&samples, 24_000);
        let duration = samples.len() as f64 / 24_000.0;

        let key = cache_key("Hello world.", "f5:sarah:0.85:1.5");
        store_in(&dir, &key, "Hello world.", &mp3, duration);

        let (got_mp3, got_dur) = lookup_in(&dir, &key).expect("cache miss");
        assert!((got_dur - duration).abs() < 0.001);
        assert!(!got_mp3.is_empty());
        assert_eq!(got_mp3, mp3);
    }

    #[test]
    fn lookup_miss_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = cache_dir_at(&tmp.path().to_path_buf());
        assert!(lookup_in(&dir, "nonexistent").is_none());
    }

    #[test]
    fn different_voice_params_different_key() {
        let k1 = cache_key("Hello.", "f5:sarah:0.85:1.5");
        let k2 = cache_key("Hello.", "f5:sarah:0.85:2.0");
        assert_ne!(k1, k2);
    }

    #[test]
    fn same_text_and_voice_same_key() {
        let k1 = cache_key("Hello.", "f5:sarah:0.85:1.5");
        let k2 = cache_key("Hello.", "f5:sarah:0.85:1.5");
        assert_eq!(k1, k2);
    }

    #[test]
    fn encode_mp3_produces_nonempty_bytes() {
        let samples = sine_samples(4800, 24_000);
        let mp3 = encode_mp3(&samples, 24_000);
        assert!(!mp3.is_empty());
        // MP3 files start with a sync word (0xFF 0xE? or 0xFF 0xF?) or ID3 tag
        assert!(mp3[0] == 0xFF || mp3[0] == b'I');
    }
}
