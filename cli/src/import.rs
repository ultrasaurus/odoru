//! `dl import vibe <basedir>` — import vibe-synthesized segment output into
//! Odoru's document/audio store.
//!
//! See `dev/tts-backends/vibe-import.md` for the full design this
//! implements: sidecar discovery, document matching by plain-text hash,
//! the per-document/per-sentence cache-key scheme, and the
//! normalize→align→slice pipeline for turning one segment's wav into
//! per-sentence cache entries.

use std::path::{Path, PathBuf};

use anyhow::Context;
use sha2::{Digest, Sha256};
use util::documents::{self, VoiceStatus};
use util::segment_types::{Sidecar, SidecarSegment};

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn run_vibe(basedir: &Path) -> anyhow::Result<()> {
    let sidecar_path = find_sidecar(basedir)?;
    let sidecar_json = std::fs::read_to_string(&sidecar_path)
        .with_context(|| format!("reading {}", sidecar_path.display()))?;
    let sidecar: Sidecar = serde_json::from_str(&sidecar_json)
        .with_context(|| format!("parsing {}", sidecar_path.display()))?;

    let voice_id = sidecar.voice_id.clone().ok_or_else(|| {
        anyhow::anyhow!("sidecar has no voice_id — no segment has been synthesized yet")
    })?;

    let doc_id = find_or_create_document(&sidecar)?;
    // Document id is embedded in the voice id itself so the existing
    // per-sentence (voice_id, sentence_text) lookup shape used elsewhere
    // (e.g. /voices/:voice_id/words) can find this without protocol
    // changes to carry a document id separately. See "Playback" in
    // vibe-import.md.
    let doc_voice_id = format!("{voice_id}:{doc_id}");

    let mut next_sentence_index = 0usize;
    let mut total_sentences = 0usize;
    let mut total_imported = 0usize;
    let mut total_duration = 0.0f64;
    let mut skipped: Vec<String> = Vec::new();

    for seg in &sidecar.segments {
        total_sentences += seg.sentences.len();
        match import_segment(basedir, seg, &doc_voice_id, &mut next_sentence_index) {
            Ok(result) => {
                total_imported += result.imported;
                total_duration += result.duration;
                skipped.extend(result.skipped);
            }
            Err(e) => {
                // Whole-segment failure (missing/unparseable files) — skip
                // every sentence in it, but keep the sentence-index space
                // consistent across segments so a later --segment re-import
                // still lands on the right index.
                next_sentence_index += seg.sentences.len();
                skipped.push(format!("segment {}: {e}", seg.index));
            }
        }
    }

    let status = if total_imported == 0 {
        VoiceStatus::Error
    } else if total_imported == total_sentences {
        VoiceStatus::Ready
    } else {
        VoiceStatus::InProgress
    };

    let dir = documents::documents_dir()?.join(&doc_id);
    let duration = if total_imported > 0 { Some(total_duration) } else { None };
    documents::update_voice_status_in(&dir, &doc_voice_id, status, duration, None)
        .with_context(|| format!("writing voices.json for {doc_id}"))?;

    println!("doc id: {doc_id}");
    println!("voice: {doc_voice_id}");
    println!("sentences imported: {total_imported}/{total_sentences}");
    if !skipped.is_empty() {
        println!("skipped:");
        for s in &skipped {
            println!("  - {s}");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Sidecar discovery
// ---------------------------------------------------------------------------

/// Find the single `*.segments.json` in `basedir`. Errors if there isn't
/// exactly one — same "operator always names the one they mean" philosophy
/// as `--basedir` itself elsewhere in vibe; no auto-disambiguation.
fn find_sidecar(basedir: &Path) -> anyhow::Result<PathBuf> {
    let mut matches = Vec::new();
    for entry in std::fs::read_dir(basedir)
        .with_context(|| format!("reading directory {}", basedir.display()))?
    {
        let path = entry?.path();
        if path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(".segments.json")) {
            matches.push(path);
        }
    }
    match matches.len() {
        0 => anyhow::bail!("no *.segments.json found in {}", basedir.display()),
        1 => Ok(matches.remove(0)),
        _ => {
            let names: Vec<String> = matches
                .iter()
                .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_string))
                .collect();
            anyhow::bail!(
                "multiple *.segments.json found in {}: {} — pass a basedir containing exactly one",
                basedir.display(),
                names.join(", ")
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Document matching
// ---------------------------------------------------------------------------

fn sha256_hex(text: &str) -> String {
    let mut h = Sha256::new();
    h.update(text.as_bytes());
    format!("{:x}", h.finalize())
}

/// Find an existing document whose current plain text hashes to the
/// sidecar's `source_sha256`, or create one from `<repo>/data/<source_document>`.
fn find_or_create_document(sidecar: &Sidecar) -> anyhow::Result<String> {
    for doc in documents::list_all()? {
        if let Some(full) = documents::lookup_by_id(&doc.id)? {
            if sha256_hex(&full.plain_text) == sidecar.source_sha256 {
                return Ok(doc.id);
            }
        }
    }

    let data_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("data");
    let source_path = data_dir.join(&sidecar.source_document);
    let source_text = std::fs::read_to_string(&source_path)
        .with_context(|| format!("reading {}", source_path.display()))?;

    if sha256_hex(&source_text) != sidecar.source_sha256 {
        anyhow::bail!(
            "{} does not match the sidecar's source_sha256 — the source document \
             may have changed since vibe split it",
            source_path.display()
        );
    }

    let title = Path::new(&sidecar.source_document)
        .file_stem()
        .and_then(|s| s.to_str());
    documents::create_ready(title, None, &source_text, &source_text, &sidecar.source_sha256)
}

// ---------------------------------------------------------------------------
// Per-segment import
// ---------------------------------------------------------------------------

struct SegmentResult {
    imported: usize,
    duration: f64,
    skipped: Vec<String>,
}

fn import_segment(
    basedir: &Path,
    seg: &SidecarSegment,
    doc_voice_id: &str,
    next_sentence_index: &mut usize,
) -> anyhow::Result<SegmentResult> {
    let original_path = basedir.join(
        seg.files
            .original
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("segment {} has no files.original", seg.index))?,
    );
    let transcript_path = basedir.join(
        seg.files
            .transcript
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("segment {} has no files.transcript", seg.index))?,
    );
    let audio_path = basedir.join(
        seg.files
            .audio
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("segment {} has no files.audio", seg.index))?,
    );

    let original = std::fs::read_to_string(&original_path)
        .with_context(|| format!("reading {}", original_path.display()))?;

    let transcript_json = std::fs::read_to_string(&transcript_path)
        .with_context(|| format!("reading {}", transcript_path.display()))?;
    let transcript: tts::transcript::Transcript = serde_json::from_str(&transcript_json)
        .with_context(|| format!("parsing {}", transcript_path.display()))?;
    let raw_words: Vec<tts::transcript::Word> =
        transcript.segments.into_iter().flat_map(|s| s.words).collect();

    // Transcript word positions are in normalized text (numbers expanded,
    // "Speaker 1: " stripped, etc.), not in `original`'s exact substrings —
    // map them back so they line up with each sentence's literal text.
    // See "Why normalization is needed" in vibe-import.md.
    let normalized = util::normalizer::normalize_with_spans(&original);
    let words = tts::alignment::words_with_original_text(&raw_words, &normalized, &original);
    let word_ranges = locate_words_in_text(&words, &original);
    let orig_chars: Vec<char> = original.chars().collect();

    let (samples, sample_rate) = decode_wav_mono_f32(&audio_path)
        .with_context(|| format!("decoding {}", audio_path.display()))?;

    let mut search_from = 0usize;
    let mut imported = 0usize;
    let mut duration_sum = 0.0f64;
    let mut skipped = Vec::new();

    // First pass: find each sentence's raw time range (from its own aligned
    // words only — no extension yet) and a parallel skip reason where one
    // couldn't be found, so the second pass can tell "no neighbor" apart
    // from "skipped neighbor" without recomputing anything.
    let mut raw_ranges: Vec<Option<(f64, f64)>> = Vec::with_capacity(seg.sentences.len());
    let mut sent_char_ranges: Vec<Option<(usize, usize)>> = Vec::with_capacity(seg.sentences.len());
    for (i, sentence) in seg.sentences.iter().enumerate() {
        let sent_range = match find_sentence_range(&orig_chars, &sentence.text, search_from) {
            Some(r) => r,
            None => {
                skipped.push(format!(
                    "segment {} sentence {i}: text not found in segment original",
                    seg.index
                ));
                raw_ranges.push(None);
                sent_char_ranges.push(None);
                continue;
            }
        };
        search_from = sent_range.1;
        sent_char_ranges.push(Some(sent_range));

        match sentence_time_range(&word_ranges, &words, sent_range) {
            Some(t) => raw_ranges.push(Some(t)),
            None => {
                skipped.push(format!(
                    "segment {} sentence {i}: no aligned words found",
                    seg.index
                ));
                raw_ranges.push(None);
            }
        }
    }

    let mut ranges = split_gaps(&raw_ranges);
    extend_to_segment_edges(&mut ranges, samples.len() as f64 / sample_rate as f64);

    for (i, sentence) in seg.sentences.iter().enumerate() {
        let sentence_index = *next_sentence_index;
        *next_sentence_index += 1;

        let Some((start, end)) = ranges[i] else {
            continue; // already recorded in `skipped` during the first pass
        };
        // sent_char_ranges[i] is always Some when ranges[i] is — both are
        // only ever set together in the first pass.
        let sent_range = sent_char_ranges[i].expect("char range missing for a sentence with a time range");

        let start_idx = ((start * sample_rate as f64).round().max(0.0)) as usize;
        let end_idx = ((end * sample_rate as f64).round() as usize).min(samples.len());
        if end_idx <= start_idx {
            skipped.push(format!(
                "segment {} sentence {i}: empty audio range",
                seg.index
            ));
            continue;
        }

        // Cutting a sentence out of the middle of a segment's audio leaves a
        // hard edge at each boundary — almost always silent in theory, but
        // forced-alignment boundaries aren't pixel-perfect, so there's often
        // a few ms of the neighboring word/breath right at the cut, which
        // reads as an audible pop. Same fix vibe/listen-test/stitch.sh
        // applies to whole segments via ffmpeg's `afade`, just done here on
        // the in-memory sentence slice instead of shelling out per sentence.
        let mut slice = samples[start_idx..end_idx].to_vec();
        apply_fade(&mut slice, FADE_SECS, sample_rate);

        // The player chains sentence clips with zero gap (gapless
        // AudioQueue) — any pause between sentences has to be real silent
        // samples baked into the audio itself, not server-side scheduling
        // metadata (which only affects transcript timestamps used for
        // highlighting, never what's actually heard). Only paragraph
        // breaks get a deliberate pause; ordinary sentence boundaries rely
        // on whatever natural gap `split_gaps` already preserved.
        let mut paragraph_pause_secs = 0.0;
        if sentence.paragraph_end {
            let pause_samples = (PARAGRAPH_PAUSE_SECS * sample_rate as f64).round() as usize;
            slice.extend(std::iter::repeat(0.0f32).take(pause_samples));
            paragraph_pause_secs = PARAGRAPH_PAUSE_SECS;
        }

        let sentence_duration = (end - start) + paragraph_pause_secs;
        let mp3 = tts::audio_cache::encode_mp3(&slice, sample_rate);
        let key = tts::audio_cache::cache_key(
            &sentence.text,
            &format!("{doc_voice_id}:{sentence_index}"),
        );
        tts::audio_cache::store(&key, &sentence.text, &mp3, sentence_duration);

        // Rebase matched words' times to be relative to this sentence's own
        // slice — matches how per-sentence cache entries are timed elsewhere
        // (each sentence's mp3 is its own clip starting at t=0).
        let sentence_words: Vec<tts::transcript::Word> = words
            .iter()
            .zip(word_ranges.iter())
            .filter(|(_, (ws, we))| *ws < sent_range.1 && *we > sent_range.0)
            .map(|(w, _)| {
                let mut w = w.clone();
                w.start = w.start.map(|s| s - start);
                w.end = w.end.map(|e| e - start);
                w
            })
            .collect();
        if let Some(mut meta) = tts::audio_cache::read_meta(&key) {
            meta.words = Some(sentence_words);
            tts::audio_cache::write_meta(&key, &meta);
        }

        imported += 1;
        duration_sum += sentence_duration;
    }

    Ok(SegmentResult { imported, duration: duration_sum, skipped })
}

// ---------------------------------------------------------------------------
// Sentence ↔ word matching
// ---------------------------------------------------------------------------

/// For each original-text word, find its char range within `original` via
/// sequential case-insensitive content search (mirrors the technique
/// `words_with_original_text` already uses internally). Advancing the
/// cursor per match keeps repeated words from re-matching an earlier
/// occurrence.
fn locate_words_in_text(words: &[tts::transcript::Word], original: &str) -> Vec<(usize, usize)> {
    let orig_chars: Vec<char> = original.chars().collect();
    let mut cursor = 0usize;
    let mut ranges = Vec::with_capacity(words.len());

    for w in words {
        let target: Vec<char> = w.word.chars().collect();
        if target.is_empty() {
            ranges.push((cursor, cursor));
            continue;
        }

        let mut found = None;
        let mut i = cursor;
        while i + target.len() <= orig_chars.len() {
            if orig_chars[i..i + target.len()]
                .iter()
                .zip(target.iter())
                .all(|(a, b)| a.to_lowercase().eq(b.to_lowercase()))
            {
                found = Some(i);
                break;
            }
            i += 1;
        }

        match found {
            Some(start) => {
                let end = start + target.len();
                cursor = end;
                ranges.push((start, end));
            }
            None => ranges.push((cursor, cursor)),
        }
    }

    ranges
}

/// Find `sentence_text`'s exact char range within `orig_chars`, searching
/// forward from `search_from`. Sentences are exact substrings of the
/// segment's original text by construction (see vibe's `segment.rs`), so
/// this is a plain case-sensitive search, not a fuzzy one.
fn find_sentence_range(
    orig_chars: &[char],
    sentence_text: &str,
    search_from: usize,
) -> Option<(usize, usize)> {
    let target: Vec<char> = sentence_text.chars().collect();
    if target.is_empty() {
        return None;
    }
    let mut i = search_from;
    while i + target.len() <= orig_chars.len() {
        if orig_chars[i..i + target.len()] == target[..] {
            return Some((i, i + target.len()));
        }
        i += 1;
    }
    None
}

/// Time span covering every word whose char range overlaps the sentence's.
fn sentence_time_range(
    word_ranges: &[(usize, usize)],
    words: &[tts::transcript::Word],
    sent_range: (usize, usize),
) -> Option<(f64, f64)> {
    let mut start: Option<f64> = None;
    let mut end: Option<f64> = None;

    for (w, (ws, we)) in words.iter().zip(word_ranges.iter()) {
        if *ws < sent_range.1 && *we > sent_range.0 {
            if let Some(s) = w.start {
                start = Some(start.map_or(s, |cur: f64| cur.min(s)));
            }
            if let Some(e) = w.end {
                end = Some(end.map_or(e, |cur: f64| cur.max(e)));
            }
        }
    }

    match (start, end) {
        (Some(s), Some(e)) if e > s => Some((s, e)),
        _ => None,
    }
}

/// Each sentence's raw range stops right at its own first/last aligned
/// word, discarding whatever natural pause separated it from its neighbor
/// in the original audio. Split that gap in half between adjacent
/// sentences rather than handing all of it to neither — mirrors the
/// pre-roll/safety-margin halving `annotations.ts`'s
/// `findAnnotationWordRange` already does between neighboring words.
///
/// Only extends across an immediate neighbor that itself has a valid
/// range — a skipped sentence (`None`) in between leaves both its
/// neighbors' edges untouched, and a negative or zero gap (overlapping
/// ranges) is left as-is rather than being "fixed" into something smaller.
fn split_gaps(raw_ranges: &[Option<(f64, f64)>]) -> Vec<Option<(f64, f64)>> {
    let mut ranges = raw_ranges.to_vec();
    for i in 0..raw_ranges.len().saturating_sub(1) {
        if let (Some((_, cur_end)), Some((next_start, _))) = (raw_ranges[i], raw_ranges[i + 1]) {
            let gap = next_start - cur_end;
            if gap > 0.0 {
                let mid = cur_end + gap / 2.0;
                // Only touch the end of `ranges[i]` and the start of
                // `ranges[i + 1]` — each sentence's start is set exactly
                // once (by the iteration where it's the "next" neighbor)
                // and its end is set exactly once (by the iteration where
                // it's "cur"), so a left-to-right pass can't clobber an
                // extension a sentence already received from its other
                // neighbor.
                if let Some((s, _)) = ranges[i] {
                    ranges[i] = Some((s, mid));
                }
                if let Some((_, e)) = ranges[i + 1] {
                    ranges[i + 1] = Some((mid, e));
                }
            }
        }
    }
    ranges
}

/// Unlike interior sentences, the first and last sentence in a segment have
/// no neighbor on their outward side for `split_gaps` to borrow slack from
/// — their outward edge sits exactly at their own first/last aligned word's
/// timestamp, with no margin. `apply_fade`'s fade-out/in then eats straight
/// into that word's real audio instead of silence, audibly clipping it.
/// Push the first valid sentence's start back to `0.0` and the last valid
/// sentence's end out to `segment_duration` so the fade always has the
/// segment's own leading/trailing silence (or breath) to work with.
fn extend_to_segment_edges(ranges: &mut [Option<(f64, f64)>], segment_duration: f64) {
    if let Some(i) = ranges.iter().position(|r| r.is_some()) {
        if let Some((_, e)) = ranges[i] {
            ranges[i] = Some((0.0, e));
        }
    }
    if let Some(i) = ranges.iter().rposition(|r| r.is_some()) {
        if let Some((s, _)) = ranges[i] {
            ranges[i] = Some((s, segment_duration));
        }
    }
}

// ---------------------------------------------------------------------------
// Fade in/out
// ---------------------------------------------------------------------------

/// Matches `vibe/listen-test/stitch.sh`'s default `afade` duration.
const FADE_SECS: f64 = 0.15;

/// Deliberate pause appended after a sentence that ends a paragraph,
/// baked into the stored audio (see the gapless-playback note where this
/// is used) — not the same kind of gap `split_gaps` recovers between
/// ordinary sentences, which only restores a *natural* pause that was
/// already in the recording.
const PARAGRAPH_PAUSE_SECS: f64 = 0.8;

/// Linear fade-in/fade-out at the start/end of `samples`, in place. Cutting
/// a sentence out of a larger segment leaves a hard edge at each boundary —
/// alignment timestamps aren't sample-accurate, so there's often a sliver of
/// a neighboring word or breath sound right at the cut, which reads as an
/// audible pop without this.
fn apply_fade(samples: &mut [f32], fade_secs: f64, sample_rate: u32) {
    let len = samples.len();
    if len < 2 {
        return;
    }
    // Cap at half the clip so fade-in and fade-out can't overlap on a very
    // short sentence.
    let fade_len = ((fade_secs * sample_rate as f64).round() as usize).min(len / 2);
    if fade_len == 0 {
        return;
    }

    for i in 0..fade_len {
        let gain = i as f32 / fade_len as f32;
        samples[i] *= gain;
    }
    for i in 0..fade_len {
        let gain = i as f32 / fade_len as f32;
        samples[len - 1 - i] *= gain;
    }
}

// ---------------------------------------------------------------------------
// Wav decoding
// ---------------------------------------------------------------------------

/// Decode a mono wav file to f32 PCM samples in [-1.0, 1.0] plus its sample
/// rate. Errors on anything not mono — vibe's output is mono by design, and
/// silently downmixing a stereo file would hide a real assumption violation.
fn decode_wav_mono_f32(path: &Path) -> anyhow::Result<(Vec<f32>, u32)> {
    let mut reader = hound::WavReader::open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    let spec = reader.spec();
    if spec.channels != 1 {
        anyhow::bail!("{} is not mono ({} channels)", path.display(), spec.channels);
    }

    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let max = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / max))
                .collect::<Result<_, _>>()
                .with_context(|| format!("decoding samples from {}", path.display()))?
        }
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<Result<_, _>>()
            .with_context(|| format!("decoding samples from {}", path.display()))?,
    };

    Ok((samples, spec.sample_rate))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tts::transcript::Word;

    // ── apply_fade ────────────────────────────────────────────────────────

    #[test]
    fn apply_fade_ramps_start_and_end_to_zero() {
        let mut samples = vec![1.0f32; 100];
        apply_fade(&mut samples, 0.1, 1000); // fade_len = 100 samples, but capped to 50
        assert_eq!(samples[0], 0.0);
        assert_eq!(samples[samples.len() - 1], 0.0);
    }

    #[test]
    fn apply_fade_leaves_middle_untouched() {
        let mut samples = vec![1.0f32; 1000];
        apply_fade(&mut samples, 0.01, 1000); // fade_len = 10 samples
        assert_eq!(samples[500], 1.0);
    }

    #[test]
    fn apply_fade_is_monotonic_ramp() {
        let mut samples = vec![1.0f32; 1000];
        apply_fade(&mut samples, 0.01, 1000); // fade_len = 10 samples
        for i in 0..9 {
            assert!(samples[i] < samples[i + 1], "fade-in should be increasing");
        }
        let len = samples.len();
        for i in 0..9 {
            assert!(samples[len - 1 - i] < samples[len - 2 - i], "fade-out should be increasing toward the middle");
        }
    }

    #[test]
    fn apply_fade_handles_clip_shorter_than_fade_window() {
        let mut samples = vec![1.0f32; 4];
        // fade_secs * sample_rate would exceed len/2 — should clamp, not panic.
        apply_fade(&mut samples, 1.0, 1000);
        assert_eq!(samples.len(), 4);
    }

    #[test]
    fn apply_fade_on_empty_or_tiny_slice_does_not_panic() {
        let mut empty: Vec<f32> = vec![];
        apply_fade(&mut empty, 0.1, 1000);
        let mut one = vec![1.0f32];
        apply_fade(&mut one, 0.1, 1000);
    }

    fn word(text: &str, start: f64, end: f64) -> Word {
        Word {
            word: text.to_string(),
            start: Some(start),
            end: Some(end),
            score: Some(1.0),
            speaker: None,
        }
    }

    // ── find_sentence_range ───────────────────────────────────────────────

    #[test]
    fn find_sentence_range_finds_first_occurrence() {
        let text: Vec<char> = "Hello world. Hello again.".chars().collect();
        let r = find_sentence_range(&text, "Hello", 0).unwrap();
        assert_eq!(r, (0, 5));
    }

    #[test]
    fn find_sentence_range_advances_past_search_from() {
        let text: Vec<char> = "Hello world. Hello again.".chars().collect();
        let r = find_sentence_range(&text, "Hello", 5).unwrap();
        assert_eq!(&text[r.0..r.1].iter().collect::<String>(), "Hello");
        assert!(r.0 > 5);
    }

    #[test]
    fn find_sentence_range_is_case_sensitive() {
        let text: Vec<char> = "hello world.".chars().collect();
        assert!(find_sentence_range(&text, "Hello", 0).is_none());
    }

    #[test]
    fn find_sentence_range_missing_returns_none() {
        let text: Vec<char> = "Hello world.".chars().collect();
        assert!(find_sentence_range(&text, "Goodbye", 0).is_none());
    }

    // ── locate_words_in_text ──────────────────────────────────────────────

    #[test]
    fn locate_words_in_text_sequential_match() {
        let original = "Item 71279 was filed.";
        let words = vec![
            word("Item", 0.0, 0.3),
            word("71279", 0.3, 1.0),
            word("was", 1.0, 1.2),
            word("filed", 1.2, 1.8),
        ];
        let ranges = locate_words_in_text(&words, original);
        let texts: Vec<String> = ranges
            .iter()
            .map(|(s, e)| original.chars().skip(*s).take(e - s).collect())
            .collect();
        assert_eq!(texts, vec!["Item", "71279", "was", "filed"]);
    }

    #[test]
    fn locate_words_in_text_is_case_insensitive() {
        let original = "HELLO world";
        let words = vec![word("hello", 0.0, 0.5)];
        let ranges = locate_words_in_text(&words, original);
        assert_eq!(ranges[0], (0, 5));
    }

    #[test]
    fn locate_words_in_text_repeated_word_advances_cursor() {
        let original = "the cat and the dog";
        let words = vec![word("the", 0.0, 0.1), word("the", 1.0, 1.1)];
        let ranges = locate_words_in_text(&words, original);
        assert_eq!(ranges[0], (0, 3));
        assert_eq!(ranges[1], (12, 15));
    }

    #[test]
    fn locate_words_in_text_unmatched_word_falls_back_to_cursor() {
        let original = "hello world";
        let words = vec![word("nonexistent", 0.0, 0.5), word("world", 0.5, 1.0)];
        let ranges = locate_words_in_text(&words, original);
        assert_eq!(ranges[0], (0, 0));
        // cursor didn't advance, so "world" is still found from position 0.
        assert_eq!(ranges[1], (6, 11));
    }

    // ── sentence_time_range ───────────────────────────────────────────────

    #[test]
    fn sentence_time_range_covers_overlapping_words() {
        let words = vec![word("Item", 0.0, 0.3), word("71279", 0.3, 1.0), word("was", 1.0, 1.2)];
        let ranges = vec![(0, 4), (5, 10), (11, 14)];
        // sentence covers chars [0, 10) — "Item 71279"
        let r = sentence_time_range(&ranges, &words, (0, 10)).unwrap();
        assert_eq!(r, (0.0, 1.0));
    }

    #[test]
    fn sentence_time_range_no_overlap_returns_none() {
        let words = vec![word("Item", 0.0, 0.3)];
        let ranges = vec![(0, 4)];
        assert!(sentence_time_range(&ranges, &words, (10, 20)).is_none());
    }

    #[test]
    fn sentence_time_range_words_missing_timestamps_returns_none() {
        let w = Word { word: "x".into(), start: None, end: None, score: None, speaker: None };
        let ranges = vec![(0, 1)];
        assert!(sentence_time_range(&ranges, &[w], (0, 1)).is_none());
    }

    // ── split_gaps ────────────────────────────────────────────────────────

    #[test]
    fn split_gaps_meets_in_the_middle() {
        let raw = vec![Some((0.0, 1.0)), Some((1.4, 2.0))];
        let split = split_gaps(&raw);
        assert_eq!(split[0], Some((0.0, 1.2)));
        assert_eq!(split[1], Some((1.2, 2.0)));
    }

    #[test]
    fn split_gaps_leaves_touching_ranges_unchanged() {
        let raw = vec![Some((0.0, 1.0)), Some((1.0, 2.0))];
        let split = split_gaps(&raw);
        assert_eq!(split, raw);
    }

    #[test]
    fn split_gaps_leaves_overlapping_ranges_unchanged() {
        // Negative gap (overlap) — not "fixed", left as-is.
        let raw = vec![Some((0.0, 1.5)), Some((1.0, 2.0))];
        let split = split_gaps(&raw);
        assert_eq!(split, raw);
    }

    #[test]
    fn split_gaps_skips_extension_across_a_none_neighbor() {
        let raw = vec![Some((0.0, 1.0)), None, Some((2.0, 3.0))];
        let split = split_gaps(&raw);
        assert_eq!(split, raw);
    }

    #[test]
    fn split_gaps_handles_all_none() {
        let raw: Vec<Option<(f64, f64)>> = vec![None, None];
        assert_eq!(split_gaps(&raw), raw);
    }

    #[test]
    fn split_gaps_single_element_unchanged() {
        let raw = vec![Some((0.0, 1.0))];
        assert_eq!(split_gaps(&raw), raw);
    }

    #[test]
    fn split_gaps_three_in_a_row_each_meet_neighbors_halfway() {
        let raw = vec![Some((0.0, 1.0)), Some((1.2, 1.8)), Some((2.0, 3.0))];
        let split = split_gaps(&raw);
        // gap before middle: (1.0,1.2) -> mid 1.1; gap after middle: (1.8,2.0) -> mid 1.9
        assert_eq!(split[0], Some((0.0, 1.1)));
        assert_eq!(split[1], Some((1.1, 1.9)));
        assert_eq!(split[2], Some((1.9, 3.0)));
    }

    // ── extend_to_segment_edges ────────────────────────────────────────────

    #[test]
    fn extend_to_segment_edges_pushes_first_start_to_zero() {
        let mut ranges = vec![Some((0.5, 1.0)), Some((1.2, 2.0))];
        extend_to_segment_edges(&mut ranges, 3.0);
        assert_eq!(ranges[0], Some((0.0, 1.0)));
    }

    #[test]
    fn extend_to_segment_edges_pushes_last_end_to_segment_duration() {
        let mut ranges = vec![Some((0.5, 1.0)), Some((1.2, 2.0))];
        extend_to_segment_edges(&mut ranges, 3.0);
        assert_eq!(ranges[1], Some((1.2, 3.0)));
    }

    #[test]
    fn extend_to_segment_edges_single_sentence_gets_both_edges() {
        let mut ranges = vec![Some((0.5, 1.0))];
        extend_to_segment_edges(&mut ranges, 3.0);
        assert_eq!(ranges[0], Some((0.0, 3.0)));
    }

    #[test]
    fn extend_to_segment_edges_skips_leading_and_trailing_none() {
        let mut ranges = vec![None, Some((0.5, 1.0)), Some((1.2, 2.0)), None];
        extend_to_segment_edges(&mut ranges, 3.0);
        assert_eq!(ranges, vec![None, Some((0.0, 1.0)), Some((1.2, 3.0)), None]);
    }

    #[test]
    fn extend_to_segment_edges_all_none_is_noop() {
        let mut ranges: Vec<Option<(f64, f64)>> = vec![None, None];
        extend_to_segment_edges(&mut ranges, 3.0);
        assert_eq!(ranges, vec![None, None]);
    }

    // ── find_sidecar ──────────────────────────────────────────────────────

    #[test]
    fn find_sidecar_errors_when_none_found() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(find_sidecar(tmp.path()).is_err());
    }

    #[test]
    fn find_sidecar_errors_when_multiple_found() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.segments.json"), "{}").unwrap();
        std::fs::write(tmp.path().join("b.segments.json"), "{}").unwrap();
        assert!(find_sidecar(tmp.path()).is_err());
    }

    #[test]
    fn find_sidecar_finds_the_one_match() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("other.txt"), "ignored").unwrap();
        let expected = tmp.path().join("authorship.segments.json");
        std::fs::write(&expected, "{}").unwrap();
        assert_eq!(find_sidecar(tmp.path()).unwrap(), expected);
    }

    // ── sha256_hex ────────────────────────────────────────────────────────

    #[test]
    fn sha256_hex_matches_known_value() {
        // sha256("") — well-known test vector
        assert_eq!(
            sha256_hex(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    // ── decode_wav_mono_f32 ───────────────────────────────────────────────

    #[test]
    fn decode_wav_mono_f32_roundtrips_int16() {
        let tmp = tempfile::NamedTempFile::with_suffix(".wav").unwrap();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 24_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        {
            let mut writer = hound::WavWriter::create(tmp.path(), spec).unwrap();
            for v in [0i16, 16384, -16384, 32767, -32768] {
                writer.write_sample(v).unwrap();
            }
            writer.finalize().unwrap();
        }

        let (samples, sample_rate) = decode_wav_mono_f32(tmp.path()).unwrap();
        assert_eq!(sample_rate, 24_000);
        assert_eq!(samples.len(), 5);
        assert!((samples[0]).abs() < 1e-6);
        assert!((samples[1] - 0.5).abs() < 0.001);
        assert!((samples[2] + 0.5).abs() < 0.001);
    }

    #[test]
    fn decode_wav_mono_f32_rejects_stereo() {
        let tmp = tempfile::NamedTempFile::with_suffix(".wav").unwrap();
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 24_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        {
            let mut writer = hound::WavWriter::create(tmp.path(), spec).unwrap();
            writer.write_sample(0i16).unwrap();
            writer.write_sample(0i16).unwrap();
            writer.finalize().unwrap();
        }

        assert!(decode_wav_mono_f32(tmp.path()).is_err());
    }
}
