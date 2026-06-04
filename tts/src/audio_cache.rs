//! Disk cache for synthesized audio segments.
//!
//! Cache directory: `~/.odoru/audio/`
//!
//! Each entry is two files:
//!   `<hash>.f32`  — raw f32le samples
//!   `<hash>.json` — metadata (text, sample_rate, duration)
//!
//! Cache key: SHA-256(normalized_text + "|" + voice_cache_key)
//! This means changing voice params (speed, cfg_strength) busts the cache.

use std::path::PathBuf;
use std::io::{Read, Write};

use sha2::{Digest, Sha256};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Metadata sidecar
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct Meta {
    text: String,
    sample_rate: u32,
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

/// Look up a cached segment. Returns `(samples, sample_rate, duration)` on hit.
pub fn lookup(key: &str) -> Option<(Vec<f32>, u32, f64)> {
    lookup_in(&cache_dir()?, key)
}

fn lookup_in(dir: &PathBuf, key: &str) -> Option<(Vec<f32>, u32, f64)> {
    let f32_path = dir.join(format!("{key}.f32"));
    let json_path = dir.join(format!("{key}.json"));

    if !f32_path.exists() || !json_path.exists() {
        return None;
    }

    let meta: Meta = {
        let s = std::fs::read_to_string(&json_path).ok()?;
        serde_json::from_str(&s).ok()?
    };

    let samples = {
        let mut f = std::fs::File::open(&f32_path).ok()?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).ok()?;
        if buf.len() % 4 != 0 { return None; }
        buf.chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect()
    };

    Some((samples, meta.sample_rate, meta.duration))
}

/// Check whether a cache entry exists without reading the audio data.
/// Much cheaper than `lookup` — use this when you only need to know if a
/// sentence is cached, not to actually play it back.
pub fn exists(key: &str) -> bool {
    let Some(dir) = cache_dir() else { return false; };
    dir.join(format!("{key}.f32")).exists() && dir.join(format!("{key}.json")).exists()
}

/// Write a synthesized segment to the cache.
pub fn store(key: &str, text: &str, samples: &[f32], sample_rate: u32, duration: f64) {
    let Some(dir) = cache_dir() else { return; };
    store_in(&dir, key, text, samples, sample_rate, duration);
}

fn store_in(dir: &PathBuf, key: &str, text: &str, samples: &[f32], sample_rate: u32, duration: f64) {
    if let Err(e) = std::fs::create_dir_all(dir) {
        eprintln!("audio cache: failed to create dir: {e}");
        return;
    }

    let f32_path = dir.join(format!("{key}.f32"));
    let bytes: Vec<u8> = samples.iter()
        .flat_map(|s| s.to_le_bytes())
        .collect();
    if let Err(e) = std::fs::File::create(&f32_path).and_then(|mut f| f.write_all(&bytes)) {
        eprintln!("audio cache: failed to write samples: {e}");
        return;
    }

    let json_path = dir.join(format!("{key}.json"));
    let meta = Meta { text: text.to_string(), sample_rate, duration };
    if let Ok(json) = serde_json::to_string(&meta) {
        if let Err(e) = std::fs::write(&json_path, json) {
            eprintln!("audio cache: failed to write metadata: {e}");
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

    #[test]
    fn roundtrip_store_and_lookup() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = cache_dir_at(&tmp.path().to_path_buf());

        let key = cache_key("Hello world.", "f5:sarah:0.85:1.5");
        let samples = vec![0.1f32, -0.2, 0.3];
        store_in(&dir, &key, "Hello world.", &samples, 24_000, 0.5);
        let (got_samples, got_sr, got_dur) = lookup_in(&dir, &key).expect("cache miss");
        assert_eq!(got_sr, 24_000);
        assert!((got_dur - 0.5).abs() < 0.001);
        assert_eq!(got_samples.len(), 3);
        assert!((got_samples[0] - 0.1).abs() < 0.0001);
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
}
