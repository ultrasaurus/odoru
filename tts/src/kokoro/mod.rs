//! Kokoro TTS backend — ONNX inference (Rust) + G2P (Python/misaki).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{bail, Result};
use hound::{SampleFormat, WavSpec, WavWriter};
use futures::StreamExt;
use ort::{inputs, session::Session, value::Tensor};

use tracing::{info, warn};
use crate::chunk::{AudioSegment, Segment};
use crate::engine::TtsBackend;
use crate::error::TtsError;
use crate::g2p::G2pEngine;

// ---------------------------------------------------------------------------
// KokoroBackend
// ---------------------------------------------------------------------------

pub struct KokoroBackend {
    inference: Mutex<KokoroInference>,
}

impl KokoroBackend {
    /// Create backend. `g2p` must already be initialized (Python + venv setup
    /// must happen before this is called).
    pub fn init(model_dir: PathBuf, voice: &str, speed: f32, g2p: G2pEngine) -> Result<Self> {
        let inference = KokoroInference::new(model_dir, voice, speed, g2p)?;
        Ok(Self { inference: Mutex::new(inference) })
    }
}

impl TtsBackend for KokoroBackend {
    fn synthesize_sentence(
        &self,
        text: &str,
        voice: &crate::backend::Voice,
        index: usize,
    ) -> Result<(Vec<f32>, u32, f64), TtsError> {
        let voice_name = match voice {
            crate::backend::Voice::Kokoro { name } => name.as_str(),
            _ => "am_puck",
        };
        let mut inference = self.inference.lock().map_err(|e| TtsError::SynthesisFailed {
            index,
            sentence: text.to_string(),
            cause: format!("lock poisoned: {e}"),
        })?;
        inference.synthesize_with_voice(text, voice_name, index).map_err(|e| TtsError::SynthesisFailed {
            index,
            sentence: text.to_string(),
            cause: e.to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// KokoroInference
// ---------------------------------------------------------------------------

struct KokoroInference {
    session: Session,
    model_dir: PathBuf,
    speed: f32,
    vocab: std::collections::HashMap<char, usize>,
    g2p: G2pEngine,
    /// Cached max token count per voice (derived from voice file size).
    voice_max_tokens: std::collections::HashMap<String, usize>,
}

impl KokoroInference {
    fn new(model_dir: PathBuf, _voice: &str, speed: f32, g2p: G2pEngine) -> Result<Self> {
        let model_path = model_dir.join("model.onnx");
        if !model_path.exists() {
            bail!("model.onnx not found in {}.\nRun setup.sh to download it.", model_dir.display());
        }

        info!("Loading Kokoro ONNX model…");
        ort::init()
            .with_execution_providers([
                ort::execution_providers::CPUExecutionProvider::default().build(),
            ])
            .commit();

        let session = Session::builder()
            .map_err(|e| anyhow::anyhow!("Session builder: {e}"))?
            .with_execution_providers([
                ort::execution_providers::CPUExecutionProvider::default().build(),
            ])
            .map_err(|e| anyhow::anyhow!("Execution providers: {e}"))?
            .commit_from_file(&model_path)
            .map_err(|e| anyhow::anyhow!("Load model: {e}"))?;
        info!("Kokoro model ready.");

        let vocab = build_vocab(&model_dir)?;

        Ok(Self { session, model_dir, speed, vocab, g2p, voice_max_tokens: std::collections::HashMap::new() })
    }

    fn synthesize_with_voice(&mut self, text: &str, voice: &str, index: usize) -> Result<(Vec<f32>, u32, f64)> {
        let rt = tokio::runtime::Runtime::new()?;

        let phonemes = self.phonemize(&rt, text);
        if phonemes.is_empty() {
            bail!("No phonemes produced for: {text:?}");
        }

        let token_ids = tokenize(&phonemes, &self.vocab);
        if token_ids.is_empty() {
            bail!("No token IDs for phonemes: {phonemes:?}");
        }

        let max_tokens = self.max_tokens_for_voice(voice)?;

        // Fast path: sentence fits within the voice file limit.
        if token_ids.len() < max_tokens {
            let (samples, durations) = self.run_onnx(&token_ids, voice)?;
            return Ok((samples, 24_000, audio_duration_from_durations(&durations)));
        }

        // Slow path: sentence is too long — split into clauses, synthesize
        // each, and concatenate. Transparent to the caller.
        let groups = split_into_clauses(text, token_ids.len(), max_tokens);
        warn!("sentence {index} too long ({} tokens, max {max_tokens}) — split into {} groups",
              token_ids.len(), groups.len());

        const SAMPLE_RATE: u32 = 24_000;
        const SILENCE_MS: u64 = 50;
        let silence: Vec<f32> = vec![0.0; (SAMPLE_RATE as u64 * SILENCE_MS / 1000) as usize];

        let mut all_samples: Vec<f32> = Vec::new();
        let mut total_duration = 0.0f64;

        for (i, group) in groups.iter().enumerate() {
            let group_phonemes = self.phonemize(&rt, group);
            let mut group_tokens = tokenize(&group_phonemes, &self.vocab);

            // Safety clamp: if a group is still over the limit (e.g. a single
            // very long word), truncate rather than error.
            if group_tokens.len() >= max_tokens {
                warn!("sentence {index} group {i} still over limit after split ({} tokens) — clamping",
                      group_tokens.len());
                group_tokens.truncate(max_tokens - 1);
            }

            if group_tokens.is_empty() { continue; }

            let (samples, durations) = self.run_onnx(&group_tokens, voice)?;
            total_duration += audio_duration_from_durations(&durations);
            all_samples.extend_from_slice(&samples);

            if i + 1 < groups.len() {
                all_samples.extend_from_slice(&silence);
                total_duration += SILENCE_MS as f64 / 1000.0;
            }
        }

        Ok((all_samples, SAMPLE_RATE, total_duration))
    }

    /// Phonemize `text` using the G2P engine on the provided runtime.
    fn phonemize(&mut self, rt: &tokio::runtime::Runtime, text: &str) -> String {
        rt.block_on(async {
            let mut stream = self.g2p.phonemize(text);
            let mut result = String::new();
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(c) => result = c.phonemes,
                    Err(e) => warn!("G2P error: {e}"),
                }
            }
            result
        })
    }

    /// Return the max supported token count for a voice, derived from its
    /// style file size. Result is cached after the first call per voice.
    fn max_tokens_for_voice(&mut self, voice: &str) -> Result<usize> {
        if let Some(&n) = self.voice_max_tokens.get(voice) {
            return Ok(n);
        }
        let path = self.model_dir.join("voices").join(format!("{voice}.bin"));
        let byte_len = std::fs::metadata(&path)
            .map_err(|e| anyhow::anyhow!("Voice file metadata {}: {e}", path.display()))?
            .len() as usize;
        // Each row is 256 f32 values (4 bytes each).
        let max_tokens = byte_len / (256 * 4);
        self.voice_max_tokens.insert(voice.to_string(), max_tokens);
        Ok(max_tokens)
    }

    fn run_onnx(&mut self, token_ids: &[i64], voice: &str) -> Result<(Vec<f32>, Vec<f32>)> {
        let n_tokens = token_ids.len();
        let style = load_voice_style(&self.model_dir, voice, n_tokens)?;

        let mut ids = Vec::with_capacity(n_tokens + 2);
        ids.push(0i64);
        ids.extend_from_slice(token_ids);
        ids.push(0i64);
        let seq_len = ids.len();

        let input_ids_tensor =
            Tensor::<i64>::from_array(([1, seq_len], ids.into_boxed_slice()))
                .map_err(|e| anyhow::anyhow!("input_ids: {e}"))?;
        let style_tensor =
            Tensor::<f32>::from_array(([1usize, 256], style.into_boxed_slice()))
                .map_err(|e| anyhow::anyhow!("style: {e}"))?;
        let speed_tensor =
            Tensor::<f32>::from_array(([1usize], vec![self.speed].into_boxed_slice()))
                .map_err(|e| anyhow::anyhow!("speed: {e}"))?;

        let outputs = self.session
            .run(inputs![
                "input_ids" => input_ids_tensor,
                "style"     => style_tensor,
                "speed"     => speed_tensor,
            ])
            .map_err(|e| anyhow::anyhow!("ONNX inference: {e}"))?;

        let (_, waveform) = outputs["waveform"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow::anyhow!("Extract waveform: {e}"))?;
        let (_, durations) = outputs["durations"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow::anyhow!("Extract durations: {e}"))?;

        Ok((waveform.to_vec(), durations.to_vec()))
    }
}

// ---------------------------------------------------------------------------
// Long-sentence splitting
// ---------------------------------------------------------------------------

/// Split `text` into groups each estimated to be under `max_tokens`, using
/// clause boundaries as split points. `total_tokens` is the known token count
/// for the full sentence (used for proportional estimation).
fn split_into_clauses(text: &str, total_tokens: usize, max_tokens: usize) -> Vec<String> {
    let pieces = split_on_clause_boundaries(text);
    let text_len = text.len().max(1);

    let mut groups: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_est = 0usize;

    for piece in pieces {
        // Proportional token estimate for this piece.
        let piece_est = (piece.len() as f64 / text_len as f64 * total_tokens as f64).ceil() as usize;

        // If the piece alone exceeds max_tokens, word-split it first.
        if piece_est >= max_tokens {
            // Flush current accumulator.
            if !current.trim().is_empty() {
                groups.push(current.trim().to_string());
                current = String::new();
                current_est = 0;
            }
            groups.extend(word_split(&piece, piece_est, max_tokens));
            continue;
        }

        if !current.is_empty() && current_est + piece_est >= max_tokens {
            groups.push(current.trim().to_string());
            current = piece.to_string();
            current_est = piece_est;
        } else {
            current.push_str(&piece);
            current_est += piece_est;
        }
    }
    if !current.trim().is_empty() {
        groups.push(current.trim().to_string());
    }

    if groups.is_empty() {
        groups.push(text.to_string());
    }
    groups
}

/// Split `text` into roughly equal word-count chunks, each estimated under
/// `max_tokens`. Used as a fallback when a single clause is still too long.
fn word_split(text: &str, estimated_tokens: usize, max_tokens: usize) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() { return vec![]; }
    let n_chunks = (estimated_tokens + max_tokens - 1) / max_tokens; // ceil
    let chunk_size = (words.len() + n_chunks - 1) / n_chunks;
    words.chunks(chunk_size)
        .map(|w| w.join(" "))
        .collect()
}

/// Split `text` at clause boundaries, keeping each separator attached to the
/// preceding piece. Returns the pieces in order; joining them reconstructs
/// the original text.
fn split_on_clause_boundaries(text: &str) -> Vec<String> {
    // Separators tried in order. Each is a string we split *after*, so the
    // separator stays with the left piece.
    // Multi-char conjunctions split *before* the keyword (e.g. " and ") so
    // the right piece starts with the conjunction — more natural to speak.
    let after: &[&str]  = &["; ", "— ", "– ", ": ", ", "];
    let before: &[&str] = &[" and ", " but ", " or ", " nor ",
                             " which ", " that ", " while ", " where ", " when "];

    // Collect all split points as byte offsets where the right piece starts.
    let mut split_at: Vec<usize> = Vec::new();

    for sep in after {
        let mut pos = 0;
        while let Some(found) = text[pos..].find(sep) {
            let abs = pos + found + sep.len();
            if abs < text.len() { split_at.push(abs); }
            pos += found + sep.len();
        }
    }
    for sep in before {
        let mut pos = 0;
        while let Some(found) = text[pos..].find(sep) {
            // Split before the leading space of the separator.
            let abs = pos + found;
            if abs > 0 && abs < text.len() { split_at.push(abs); }
            pos += found + sep.len();
        }
    }

    split_at.sort_unstable();
    split_at.dedup();

    if split_at.is_empty() {
        return vec![text.to_string()];
    }

    let mut pieces = Vec::new();
    let mut prev = 0;
    for &at in &split_at {
        if at > prev {
            pieces.push(text[prev..at].to_string());
        }
        prev = at;
    }
    if prev < text.len() {
        pieces.push(text[prev..].to_string());
    }
    pieces
}

// ---------------------------------------------------------------------------
// Duration → segment timestamp
// ---------------------------------------------------------------------------

pub fn durations_to_segment(text: &str, durations: &[f32], time_offset: f64) -> Segment {
    let hf = |v: f32| (v * 2.0 / 80.0) as f64;
    let (start, end) = if durations.len() >= 3 {
        let start = time_offset + hf(durations[0]);
        let phoneme_dur: f32 = durations[1..durations.len() - 1].iter().sum();
        let end = start + hf(phoneme_dur);
        (round3(start), round3(end))
    } else {
        (time_offset, time_offset)
    };
    Segment { start, end, text: text.to_string() }
}

fn audio_duration_from_durations(durations: &[f32]) -> f64 {
    let total: f32 = durations.iter().sum();
    (total * 2.0 / 80.0) as f64
}

fn round3(x: f64) -> f64 { (x * 1000.0).round() / 1000.0 }

// ---------------------------------------------------------------------------
// WAV helpers
// ---------------------------------------------------------------------------

pub fn save_wav_all(
    segments: &[AudioSegment],
    path: impl AsRef<Path>,
    gap_ms: u32,
) -> Result<()> {
    let path = path.as_ref();
    if segments.is_empty() { bail!("No segments to write"); }
    let sample_rate = segments[0].sample_rate;
    let silence_len = (sample_rate * gap_ms / 1000) as usize;
    let spec = WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut writer = WavWriter::create(path, spec)
        .map_err(|e| anyhow::anyhow!("Create WAV {}: {e}", path.display()))?;
    for (i, seg) in segments.iter().enumerate() {
        for &s in &seg.samples {
            let pcm = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            writer.write_sample(pcm).map_err(|e| anyhow::anyhow!("WAV write: {e}"))?;
        }
        if i + 1 < segments.len() {
            for _ in 0..silence_len {
                writer.write_sample(0i16).map_err(|e| anyhow::anyhow!("WAV silence: {e}"))?;
            }
        }
    }
    writer.finalize().map_err(|e| anyhow::anyhow!("WAV finalize: {e}"))
}

// ---------------------------------------------------------------------------
// Vocab / tokenizer
// ---------------------------------------------------------------------------

pub fn build_vocab(model_dir: &Path) -> Result<std::collections::HashMap<char, usize>> {
    let path = model_dir.join("tokenizer.json");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("Read {}: {e}", path.display()))?;
    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| anyhow::anyhow!("Parse tokenizer.json: {e}"))?;
    let vocab_obj = json.get("model").and_then(|m| m.get("vocab"))
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow::anyhow!("tokenizer.json missing model.vocab"))?;
    let mut map = std::collections::HashMap::new();
    for (token, id_val) in vocab_obj {
        let id = id_val.as_u64()
            .ok_or_else(|| anyhow::anyhow!("Non-integer ID for {token:?}"))? as usize;
        let mut chars = token.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) { map.insert(c, id); }
    }
    Ok(map)
}

pub fn tokenize(phonemes: &str, vocab: &std::collections::HashMap<char, usize>) -> Vec<i64> {
    phonemes.chars().filter_map(|c| vocab.get(&c).map(|&id| id as i64)).collect()
}

fn load_voice_style(model_dir: &Path, voice: &str, n_tokens: usize) -> Result<Vec<f32>> {
    let path = model_dir.join("voices").join(format!("{}.bin", voice));
    if !path.exists() { bail!("Voice file not found: {}", path.display()); }
    let bytes = std::fs::read(&path).map_err(|e| anyhow::anyhow!("Read voice: {e}"))?;
    let floats: Vec<f32> = bytes.chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    let offset = n_tokens * 256;
    if offset + 256 > floats.len() { bail!("Voice file too short for {n_tokens} tokens"); }
    Ok(floats[offset..offset + 256].to_vec())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn inline_vocab() -> std::collections::HashMap<char, usize> {
        [('h', 50usize), ('z', 68), ('\u{00f0}', 81), (' ', 16)].into_iter().collect()
    }

    #[test]
    fn tokenize_known_chars() { assert_eq!(tokenize("h", &inline_vocab()), vec![50]); }
    #[test]
    fn tokenize_skips_unknown() { assert_eq!(tokenize("h\x00z", &inline_vocab()), vec![50, 68]); }
    #[test]
    fn tokenize_th_sound() { assert_eq!(tokenize("\u{00f0}", &inline_vocab()), vec![81]); }
    #[test]
    fn tokenize_all_unknown_returns_empty() { assert!(tokenize("\x01\x02\x03", &inline_vocab()).is_empty()); }
    #[test]
    fn tokenize_mixed_preserves_order() { assert_eq!(tokenize("h\x01z\x02h", &inline_vocab()), vec![50, 68, 50]); }
    #[test]
    fn tokenize_empty_returns_empty() { assert!(tokenize("", &inline_vocab()).is_empty()); }

    #[test]
    fn build_vocab_from_real_file() {
        let model_dir = match std::env::var("KOKORO_MODEL_DIR") {
            Ok(d) => PathBuf::from(d),
            Err(_) => { warn!("Skipping Kokoro: $KOKORO_MODEL_DIR not set"); return; }
        };
        let vocab = build_vocab(&model_dir).expect("build_vocab failed");
        assert_eq!(vocab.get(&'\u{00f0}'), Some(&81));
        assert_eq!(vocab.get(&' '),        Some(&16));
        assert_eq!(vocab.get(&'.'),        Some(&4));
        assert_eq!(vocab.get(&'\u{02c8}'), Some(&156));
        assert_eq!(vocab.get(&'\u{0254}'), Some(&76));
    }

    #[test]
    fn durations_to_segment_basic() {
        let seg = durations_to_segment("hello", &[10.0, 20.0, 20.0, 5.0], 0.0);
        assert!((seg.start - 0.25).abs() < 0.001);
        assert!((seg.end - 1.25).abs() < 0.001);
    }

    #[test]
    fn durations_to_segment_with_offset() {
        let seg = durations_to_segment("hello", &[10.0, 20.0, 20.0, 5.0], 1.0);
        assert!((seg.start - 1.25).abs() < 0.001);
        assert!((seg.end - 2.25).abs() < 0.001);
    }

    #[test]
    fn durations_to_segment_too_short() {
        let seg = durations_to_segment("x", &[1.0, 2.0], 0.5);
        assert_eq!(seg.start, 0.5);
        assert_eq!(seg.end, 0.5);
    }

    // ── split_on_clause_boundaries ────────────────────────────────────────

    #[test]
    fn split_boundaries_semicolon() {
        let pieces = split_on_clause_boundaries("one; two; three");
        assert_eq!(pieces, vec!["one; ", "two; ", "three"]);
    }

    #[test]
    fn split_boundaries_comma() {
        let pieces = split_on_clause_boundaries("a, b, c");
        assert_eq!(pieces, vec!["a, ", "b, ", "c"]);
    }

    #[test]
    fn split_boundaries_conjunction() {
        // " and " splits before the conjunction, so right piece starts with "and"
        let pieces = split_on_clause_boundaries("alpha and beta");
        assert_eq!(pieces, vec!["alpha", " and beta"]);
    }

    #[test]
    fn split_boundaries_no_boundaries_returns_whole() {
        let pieces = split_on_clause_boundaries("no split here");
        assert_eq!(pieces, vec!["no split here"]);
    }

    #[test]
    fn split_boundaries_preserves_roundtrip() {
        let text = "first, second; third and fourth";
        let pieces = split_on_clause_boundaries(text);
        assert_eq!(pieces.join(""), text);
    }

    // ── word_split ────────────────────────────────────────────────────────

    #[test]
    fn word_split_divides_evenly() {
        // 6 words, estimated 200 tokens, max 100 → 2 chunks of 3 words each
        let chunks = word_split("a b c d e f", 200, 100);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], "a b c");
        assert_eq!(chunks[1], "d e f");
    }

    #[test]
    fn word_split_single_chunk_when_under_limit() {
        let chunks = word_split("hello world", 50, 100);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "hello world");
    }

    // ── split_into_clauses ────────────────────────────────────────────────

    #[test]
    fn split_into_clauses_short_text_unchanged() {
        // total_tokens < max_tokens → single group
        let groups = split_into_clauses("A short sentence.", 10, 500);
        assert_eq!(groups, vec!["A short sentence."]);
    }

    #[test]
    fn split_into_clauses_splits_at_comma() {
        // Craft a sentence where comma pieces are each ~60% of max_tokens,
        // so they can't be merged without exceeding the limit.
        // total_tokens=200, max=120. Each half ~100 tokens → must split.
        let text = "first clause here, second clause here";
        let groups = split_into_clauses(text, 200, 120);
        assert_eq!(groups.len(), 2);
        assert!(groups[0].contains("first"));
        assert!(groups[1].contains("second"));
    }

    #[test]
    fn split_into_clauses_falls_back_to_word_split() {
        // A single piece with no clause boundaries that is too long.
        let words: Vec<_> = (0..20).map(|i| format!("word{i}")).collect();
        let text = words.join(" ");
        // total=600, max=200 → needs word splitting
        let groups = split_into_clauses(&text, 600, 200);
        assert!(groups.len() >= 2, "should produce multiple groups");
        // Reassembled text should contain all words
        let rejoined = groups.join(" ");
        for w in &words { assert!(rejoined.contains(w.as_str())); }
    }
}
