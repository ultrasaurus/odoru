//! Lazy word-level forced alignment for cached audio segments.
//!
//! Word timestamps are generated on first request and stored back into the
//! audio cache sidecar so subsequent calls are instant disk reads.

use anyhow::{Context, Result};
use forced_alignment::transcript::Word;

use crate::audio_cache;

/// Return word-level timestamps for a cached audio segment, running forced
/// alignment if they are not yet stored.
///
/// `key` is the audio cache key (SHA-256 of text + "|" + voice_cache_key).
/// The ground-truth text comes from the sidecar's `meta.text` field, so the
/// caller doesn't need to supply it separately.
///
/// Returns an error if the cache entry is absent, invalid, or alignment fails.
pub fn ensure_words(key: &str) -> Result<Vec<Word>> {
    let mut meta = audio_cache::read_meta(key)
        .with_context(|| format!("no cache entry for key {key}"))?;

    if meta.invalid {
        anyhow::bail!("cache entry {key} is marked invalid");
    }

    // Fast path: already stored.
    if let Some(words) = meta.words {
        return Ok(words);
    }

    // Slow path: decode MP3 → resample to 16 kHz → align.
    let mp3 = audio_cache::mp3_path(key)
        .with_context(|| "cannot determine mp3 path")?;

    let samples = forced_alignment::audio::load_audio(&mp3, forced_alignment::SAMPLE_RATE)
        .with_context(|| format!("failed to decode {}", mp3.display()))?;

    let transcript = forced_alignment::align(&samples, &meta.text)
        .with_context(|| "forced alignment failed")?;

    let words: Vec<Word> = transcript.segments
        .into_iter()
        .flat_map(|s| s.words)
        .collect();

    // Persist back to sidecar so the next call is a fast read.
    meta.words = Some(words.clone());
    audio_cache::write_meta(key, &meta);

    Ok(words)
}
