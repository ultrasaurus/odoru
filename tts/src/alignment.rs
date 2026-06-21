//! Lazy word-level forced alignment for cached audio segments.
//!
//! Word timestamps are generated on first request and stored back into the
//! audio cache sidecar so subsequent calls are instant disk reads.

use anyhow::{Context, Result};
use dashmap::DashMap;
use forced_alignment::transcript::Word;
use std::sync::{Arc, Mutex, OnceLock};
use tracing::warn;

use crate::audio_cache;

/// Per-key locks so concurrent calls for the same cache key (e.g. two
/// overlapping client requests for the same sentence) don't each run the
/// expensive decode+align step — the second waits for the first to finish
/// and persist, then hits the fast path. Mirrors `engine.rs`'s `synth_locks`
/// pattern for audio synthesis.
type AlignLocks = DashMap<String, Arc<Mutex<()>>>;

fn align_locks() -> &'static AlignLocks {
    static LOCKS: OnceLock<AlignLocks> = OnceLock::new();
    LOCKS.get_or_init(DashMap::new)
}

/// Return word-level timestamps for a cached audio segment, running forced
/// alignment if they are not yet stored.
///
/// `key` is the audio cache key (SHA-256 of text + "|" + voice_cache_key).
/// The ground-truth text comes from the sidecar's `meta.text` field, so the
/// caller doesn't need to supply it separately.
///
/// Returns an error if the cache entry is absent, invalid, or alignment fails.
pub fn ensure_words(key: &str) -> Result<Vec<Word>> {
    if let Some(words) = check_cached_words(key)? {
        return Ok(words);
    }

    let lock = align_locks()
        .entry(key.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone();
    let _guard = lock.lock().expect("alignment lock poisoned");

    // Re-check after acquiring the lock — another caller may have finished
    // the alignment (and persisted it) while we were waiting.
    if let Some(words) = check_cached_words(key)? {
        return Ok(words);
    }

    // Slow path: decode MP3 → resample to 16 kHz → align.
    let mut meta = audio_cache::read_meta(key)
        .with_context(|| format!("no cache entry for key {key}"))?;

    let mp3 = audio_cache::mp3_path(key)
        .with_context(|| "cannot determine mp3 path")?;

    let samples = forced_alignment::audio::load_audio(&mp3, forced_alignment::SAMPLE_RATE)
        .with_context(|| format!("failed to decode {}", mp3.display()))?;

    let (transcript, report) = forced_alignment::align(&samples, &meta.text)
        .with_context(|| "forced alignment failed")?;

    if !report.filtered.is_empty() {
        warn!("alignment for {key}: {} word(s) dropped before alignment (no alignable chars): {:?}",
            report.filtered.len(), report.filtered);
    }
    if !report.suspect.is_empty() {
        warn!("alignment for {key}: {} word(s) flagged low-confidence: {:?}",
            report.suspect.len(), report.suspect);
    }

    let words: Vec<Word> = transcript.segments
        .into_iter()
        .flat_map(|s| s.words)
        .collect();

    // Persist back to sidecar so the next call is a fast read.
    meta.words = Some(words.clone());
    audio_cache::write_meta(key, &meta);

    Ok(words)
}

/// Check the sidecar for already-computed word timestamps. Returns
/// `Ok(None)` if the entry exists but alignment hasn't run yet; errors if
/// the entry is absent or marked invalid.
fn check_cached_words(key: &str) -> Result<Option<Vec<Word>>> {
    let meta = audio_cache::read_meta(key)
        .with_context(|| format!("no cache entry for key {key}"))?;

    if meta.invalid {
        anyhow::bail!("cache entry {key} is marked invalid");
    }

    Ok(meta.words)
}

/// Re-map `words` (aligned against `normalized.text`) so each `.word` field
/// holds the corresponding substring of `original` instead of its
/// normalized-text form — e.g. "seven one two seven nine" becomes "71279"
/// again.
///
/// This lets a client do literal substring matching against annotation text
/// (always the original, un-normalized sentence) the same way for F5 as
/// for Kokoro, whose words are already original text since Kokoro doesn't
/// normalize before synthesis.
///
/// Multiple normalized words can map back to the *same* original span —
/// e.g. all six of "Item"/"seven"/"one"/"two"/"seven"/"nine" map to the
/// single source span "Item 71279" (per-chunk granularity; see
/// `NormalizedText::source_range`). These are merged into one output
/// entry (combining the start of the first and the end of the last),
/// rather than returned as repeated identical entries — duplicates would
/// break a client doing `indexOf` on the joined text, since it would match
/// the first (too-early) occurrence and report a too-early end time.
///
/// Word boundaries within `normalized.text` are found by sequential
/// forward content search rather than `split_whitespace()` position
/// arithmetic, since forced alignment may drop words with no alignable
/// characters (e.g. markdown artifacts, or extra words a human reader adds
/// that aren't in the source text). Searching by content naturally skips
/// over any such gaps without needing to know which positions were dropped.
pub fn words_with_original_text(
    words: &[Word],
    normalized: &util::normalizer::NormalizedText,
    original: &str,
) -> Vec<Word> {
    let norm_chars: Vec<char> = normalized.text.chars().collect();
    let mut cursor = 0;
    let mut out: Vec<Word> = Vec::new();
    let mut last_src: Option<std::ops::Range<usize>> = None;

    for w in words {
        let target: Vec<char> = w.word.chars().collect();
        let mut found = None;
        let mut i = cursor;
        while i + target.len() <= norm_chars.len() {
            if norm_chars[i..i + target.len()].iter().zip(target.iter())
                .all(|(a, b)| a.to_lowercase().eq(b.to_lowercase()))
            {
                found = Some(i);
                break;
            }
            i += 1;
        }

        let src = found.and_then(|start| {
            let end = start + target.len();
            cursor = end;
            normalized.source_range(start..end)
        });

        if let (Some(s), Some(last)) = (&src, &last_src) {
            if s == last {
                // Same source span as the previous output word — merge by
                // extending its end time rather than appending a duplicate.
                if let Some(prev) = out.last_mut() {
                    prev.end = w.end.or(prev.end);
                }
                continue;
            }
        }

        let mut mapped = w.clone();
        if let Some(s) = &src {
            if let Some(slice) = char_slice(original, s.clone()) {
                mapped.word = slice;
            }
        }
        out.push(mapped);
        last_src = src;
    }
    out
}

fn char_slice(s: &str, range: std::ops::Range<usize>) -> Option<String> {
    let chars: Vec<char> = s.chars().collect();
    if range.end > chars.len() { return None; }
    Some(chars[range].iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use util::normalizer::normalize_with_spans;

    fn word(text: &str, start: f64, end: f64) -> Word {
        Word { word: text.to_string(), start: Some(start), end: Some(end), score: None, speaker: None }
    }

    #[test]
    fn remaps_item_number_words_back_to_original_digits() {
        let original = "see Item 71279 in the report";
        let normalized = normalize_with_spans(original);
        assert_eq!(normalized.text, "see Item seven one two seven nine in the report");

        let aligned = vec![
            word("see", 0.0, 0.2),
            word("Item", 0.2, 0.5),
            word("seven", 0.5, 0.7),
            word("one", 0.7, 0.9),
            word("two", 0.9, 1.1),
            word("seven", 1.1, 1.3),
            word("nine", 1.3, 1.5),
            word("in", 1.5, 1.6),
            word("the", 1.6, 1.7),
            word("report", 1.7, 2.0),
        ];

        let mapped = words_with_original_text(&aligned, &normalized, original);
        let words: Vec<&str> = mapped.iter().map(|w| w.word.as_str()).collect();
        // All six normalized words ("Item"/"seven"/"one"/"two"/"seven"/"nine")
        // map to the same source span "Item 71279" — merged into one entry.
        assert_eq!(words, vec!["see", "Item 71279", "in", "the", "report"]);

        // Merged entry spans from "Item"'s start to "nine"'s end.
        assert_eq!(mapped[1].start, Some(0.2));
        assert_eq!(mapped[1].end, Some(1.5));
    }

    #[test]
    fn passes_through_unchanged_when_no_normalization_occurred() {
        let original = "hello world";
        let normalized = normalize_with_spans(original);
        let aligned = vec![word("hello", 0.0, 0.3), word("world", 0.3, 0.6)];

        let mapped = words_with_original_text(&aligned, &normalized, original);
        let words: Vec<&str> = mapped.iter().map(|w| w.word.as_str()).collect();
        assert_eq!(words, vec!["hello", "world"]);
    }

    #[test]
    fn skips_gap_left_by_a_dropped_word() {
        // Simulates forced alignment filtering out a word entirely (e.g. no
        // alignable characters, or an extra word a human reader added that
        // isn't in the source text) — the aligned list is missing "beta",
        // but sequential content search should still correctly locate
        // "alpha" and "gamma" rather than getting thrown off by the gap.
        let original = "alpha beta gamma";
        let normalized = normalize_with_spans(original);
        assert_eq!(normalized.text, "alpha beta gamma"); // no-op normalization

        let aligned = vec![word("alpha", 0.0, 0.3), word("gamma", 0.6, 0.9)];
        let mapped = words_with_original_text(&aligned, &normalized, original);
        let words: Vec<&str> = mapped.iter().map(|w| w.word.as_str()).collect();
        assert_eq!(words, vec!["alpha", "gamma"]);
        assert_eq!(mapped[1].start, Some(0.6)); // not confused by the skipped "beta"
    }
}
