//! # synth
//!
//! Kokoro TTS synthesis — text → WAV + sentence timestamps.
//!
//! ## Pipeline
//!
//! ```text
//! text
//!   → sentence split  (splitter.rs)
//!   → misaki-g2p      (engine.rs, PyO3)
//!   → tokenize        (vocab.rs)
//!   → Kokoro ONNX     (ort, model.onnx)
//!   → map durations → sentence boundaries
//!   → concatenate audio
//!   → hound → WAV
//!   → Vec<SentenceTimestamp>
//! ```
//!
//! ## Example
//!
//! ```no_run
//! use ko_odoru::synth::{Synthesizer, SynthConfig};
//!
//! let mut synth = Synthesizer::new("/Users/me/.kokoro", None)?;
//! let result = synth.synthesize("Hello world. How are you?", &SynthConfig::default())?;
//! result.save_wav("output.wav")?;
//! for ts in &result.sentences {
//!     println!("[{:.3}s – {:.3}s] {}", ts.start_sec, ts.end_sec, ts.sentence);
//! }
//! # Ok::<(), anyhow::Error>(())
//! ```

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use serde_json;
use futures::StreamExt;
use hound::{SampleFormat, WavSpec, WavWriter};
use ort::{inputs, session::Session, value::Tensor};

use crate::engine::G2pEngine;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Configuration for a synthesis run.
#[derive(Debug, Clone)]
pub struct SynthConfig {
    /// Voice name, e.g. `"af_heart"` or `"am_puck"`.
    pub voice: String,
    /// Playback speed multiplier (1.0 = normal).
    pub speed: f32,
}

impl Default for SynthConfig {
    fn default() -> Self {
        Self { voice: "af_heart".into(), speed: 1.0 }
    }
}

/// Timestamp for one sentence in the output audio.
#[derive(Debug, Clone)]
pub struct SentenceTimestamp {
    /// The original sentence text.
    pub sentence: String,
    /// Start time in seconds from the beginning of the WAV file.
    pub start_sec: f32,
    /// End time in seconds from the beginning of the WAV file.
    pub end_sec: f32,
}

/// Result of a synthesis run.
pub struct SynthResult {
    /// Raw f32 audio samples at 24 kHz, mono.
    pub samples: Vec<f32>,
    /// Always 24000.
    pub sample_rate: u32,
    /// One entry per sentence, in order.
    pub sentences: Vec<SentenceTimestamp>,
}

impl SynthResult {
    /// Write the audio to a 16-bit PCM WAV file.
    pub fn save_wav(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let spec = WavSpec {
            channels: 1,
            sample_rate: self.sample_rate,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut writer = WavWriter::create(path, spec)
            .map_err(|e| anyhow::anyhow!("Failed to create WAV {}: {e}", path.display()))?;
        for &s in &self.samples {
            let pcm = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            writer.write_sample(pcm)
                .map_err(|e| anyhow::anyhow!("WAV write error: {e}"))?;
        }
        writer.finalize()
            .map_err(|e| anyhow::anyhow!("WAV finalize error: {e}"))
    }
}

// ---------------------------------------------------------------------------
// Synthesizer
// ---------------------------------------------------------------------------

/// The main TTS engine. Construct once, call `synthesize` many times.
pub struct Synthesizer {
    model_dir: PathBuf,
    session: Session,
    g2p: G2pEngine,
}

impl Synthesizer {
    /// Create a new synthesizer.
    ///
    /// - `model_dir`: directory containing `model.onnx` and `voices/`.
    ///   Typically `~/.kokoro`.
    /// - `venv_path`: path to the misaki-g2p venv. If `None`, reads
    ///   `$MISAKI_VENV`.
    pub fn new(model_dir: impl AsRef<Path>, venv_path: Option<&Path>) -> Result<Self> {
        let model_dir = model_dir.as_ref().to_path_buf();
        let model_path = model_dir.join("model.onnx");

        if !model_path.exists() {
            bail!(
                "model.onnx not found in {}.\nRun setup.sh to download it.",
                model_dir.display()
            );
        }

        eprintln!("Loading ONNX model…");
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
        eprintln!("Model ready.");

        let g2p = G2pEngine::new(venv_path)
            .map_err(|e| anyhow::anyhow!("G2P engine init: {e}"))?;

        Ok(Self { model_dir, session, g2p })
    }

    /// Synthesize `text` to audio with sentence-level timestamps.
    ///
    /// Blocks until all sentences are processed.
    pub fn synthesize(&mut self, text: &str, config: &SynthConfig) -> Result<SynthResult> {
        // Run on a fresh Tokio runtime so this stays a sync call.
        // (The G2P engine uses async streams internally.)
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| anyhow::anyhow!("Tokio runtime: {e}"))?;
        rt.block_on(self.synthesize_async(text, config))
    }

    async fn synthesize_async(&mut self, text: &str, config: &SynthConfig) -> Result<SynthResult> {
        // ----------------------------------------------------------------
        // Step 1: phonemize all sentences
        // ----------------------------------------------------------------
        let mut phoneme_chunks: Vec<(String, String)> = Vec::new(); // (sentence, phonemes)
        let mut stream = self.g2p.phonemize(text);
        while let Some(result) = stream.next().await {
            match result {
                Ok(chunk) => phoneme_chunks.push((chunk.sentence, chunk.phonemes)),
                Err(e) => eprintln!("Warning: G2P failed on a sentence: {e}"),
            }
        }

        if phoneme_chunks.is_empty() {
            bail!("No phonemes produced from input text");
        }

        // ----------------------------------------------------------------
        // Step 2: synthesize each sentence and collect audio + timestamps
        // ----------------------------------------------------------------
        let vocab = build_vocab(&self.model_dir)?;
        let silence_samples = 2400usize; // 100ms gap between sentences @ 24kHz
        let silence_secs = silence_samples as f32 / 24_000.0;
        let silence = vec![0.0f32; silence_samples];

        let mut all_samples: Vec<f32> = Vec::new();
        let mut all_sentences: Vec<SentenceTimestamp> = Vec::new();
        let mut time_offset = 0.0f32;

        for (i, (sentence, phonemes)) in phoneme_chunks.iter().enumerate() {
            eprintln!("Synthesizing sentence {i}: {:?}", sentence);

            let token_ids = tokenize(&phonemes, &vocab);
            if token_ids.is_empty() {
                eprintln!("Warning: no tokens for sentence {i}, skipping");
                continue;
            }

            let (samples, durations) =
                self.run_inference(&token_ids, config)?;

            // Map durations → sentence timestamp.
            // durations layout: [BOS, phoneme_0 .. phoneme_n, EOS]
            // BOS duration contributes real audio frames (leading attack),
            // so sentence starts after BOS and ends after all phonemes.
            // EOS is excluded from the sentence end time.
            let ts = sentence_timestamp_from_durations(
                sentence,
                &durations,
                time_offset,
            );

            let chunk_secs = samples.len() as f32 / 24_000.0;
            time_offset += chunk_secs;

            all_samples.extend_from_slice(&samples);
            all_sentences.push(ts);

            // Add silence gap between sentences (but not after the last one)
            if i + 1 < phoneme_chunks.len() {
                all_samples.extend_from_slice(&silence);
                time_offset += silence_secs;
            }
        }

        Ok(SynthResult {
            samples: all_samples,
            sample_rate: 24_000,
            sentences: all_sentences,
        })
    }

    /// Run one ONNX inference pass for a single sentence's token IDs.
    /// Returns (audio_samples, duration_values).
    fn run_inference(
        &mut self,
        token_ids: &[i64],
        config: &SynthConfig,
    ) -> Result<(Vec<f32>, Vec<f32>)> {
        let n_tokens = token_ids.len();

        // Load voice style slice for this token count
        let style = self.load_voice(&config.voice, n_tokens)?;

        // Build input_ids: [BOS=0, tokens..., EOS=0]
        let mut ids = Vec::with_capacity(n_tokens + 2);
        ids.push(0i64);
        ids.extend_from_slice(token_ids);
        ids.push(0i64);
        let seq_len = ids.len();

        let input_ids_tensor =
            Tensor::<i64>::from_array(([1, seq_len], ids.into_boxed_slice()))
                .map_err(|e| anyhow::anyhow!("input_ids tensor: {e}"))?;
        let style_tensor =
            Tensor::<f32>::from_array(([1usize, 256], style.into_boxed_slice()))
                .map_err(|e| anyhow::anyhow!("style tensor: {e}"))?;
        let speed_tensor =
            Tensor::<f32>::from_array(([1usize], vec![config.speed].into_boxed_slice()))
                .map_err(|e| anyhow::anyhow!("speed tensor: {e}"))?;

        let outputs = self.session.run(inputs![
            "input_ids" => input_ids_tensor,
            "style"     => style_tensor,
            "speed"     => speed_tensor,
        ])
        .map_err(|e| anyhow::anyhow!("ONNX inference: {e}"))?;

        let (_, waveform_data) = outputs["waveform"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow::anyhow!("Extract waveform: {e}"))?;

        let (_, duration_data) = outputs["durations"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow::anyhow!("Extract durations: {e}"))?;

        Ok((waveform_data.to_vec(), duration_data.to_vec()))
    }

    /// Load a 256-element style slice from a voice .bin file.
    /// The slice is indexed by n_tokens — each token count maps to a
    /// different style embedding row.
    fn load_voice(&self, voice: &str, n_tokens: usize) -> Result<Vec<f32>> {
        let path = self.model_dir.join("voices").join(format!("{}.bin", voice));
        if !path.exists() {
            bail!(
                "Voice file not found: {}\n\
                 Download from: https://huggingface.co/onnx-community/\
                 Kokoro-82M-v1.0-ONNX-timestamped/tree/main/voices",
                path.display()
            );
        }

        let bytes = std::fs::read(&path)
            .map_err(|e| anyhow::anyhow!("Read voice {}: {e}", path.display()))?;
        let floats: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();

        let offset = n_tokens * 256;
        if offset + 256 > floats.len() {
            bail!(
                "Voice file too short for {n_tokens} tokens \
                 (need offset {}, have {} floats)",
                offset + 256,
                floats.len()
            );
        }

        Ok(floats[offset..offset + 256].to_vec())
    }
}

// ---------------------------------------------------------------------------
// Timestamp mapping
// ---------------------------------------------------------------------------

/// Convert a `durations` tensor into a `SentenceTimestamp`.
///
/// Duration layout: `[BOS, phoneme_0, ..., phoneme_n, EOS]`
/// Units: half-frames at 80 frames/sec → seconds = value * 2 / 80
///
/// BOS generates real leading audio (soft attack), so the sentence
/// starts after BOS and ends after the last phoneme (before EOS).
fn sentence_timestamp_from_durations(
    sentence: &str,
    durations: &[f32],
    time_offset: f32,
) -> SentenceTimestamp {
    // Need at least [BOS, one phoneme, EOS]
    if durations.len() < 3 {
        return SentenceTimestamp {
            sentence: sentence.to_string(),
            start_sec: time_offset,
            end_sec: time_offset,
        };
    }

    let half_frames_to_secs = |hf: f32| hf * 2.0 / 80.0;

    // Sentence starts after BOS
    let start_sec = time_offset + half_frames_to_secs(durations[0]);

    // Sentence ends after all phoneme tokens (exclude EOS)
    let phoneme_dur: f32 = durations[1..durations.len() - 1].iter().sum();
    let end_sec = start_sec + half_frames_to_secs(phoneme_dur);

    SentenceTimestamp {
        sentence: sentence.to_string(),
        start_sec: round3(start_sec),
        end_sec: round3(end_sec),
    }
}

fn round3(x: f32) -> f32 {
    (x * 1000.0).round() / 1000.0
}

// ---------------------------------------------------------------------------
// Vocab / tokenizer
// ---------------------------------------------------------------------------

/// Load Kokoro's phoneme → token ID map from `tokenizer.json`.
///
/// Uses the file directly rather than a hardcoded list — this is the only
/// reliable way to get correct IDs, since the file uses non-sequential IDs
/// (e.g. space=16, ˈ=156, ð=81) that don't match a simple enumeration.
pub fn build_vocab(model_dir: &Path) -> Result<std::collections::HashMap<char, usize>> {
    let path = model_dir.join("tokenizer.json");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("Read {}: {e}", path.display()))?;

    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| anyhow::anyhow!("Parse tokenizer.json: {e}"))?;

    let vocab_obj = json
        .get("model")
        .and_then(|m| m.get("vocab"))
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow::anyhow!("tokenizer.json missing model.vocab"))?;

    let mut map = std::collections::HashMap::new();
    for (token, id_val) in vocab_obj {
        let id = id_val
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("Non-integer ID for {token:?}"))? as usize;
        let mut chars = token.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            map.insert(c, id);
        }
    }

    Ok(map)
}

/// Convert a phoneme string to token IDs, skipping unknown characters.
pub fn tokenize(phonemes: &str, vocab: &std::collections::HashMap<char, usize>) -> Vec<i64> {
    phonemes
        .chars()
        .filter_map(|c| vocab.get(&c).map(|&id| id as i64))
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a small inline vocab for unit tests — no file I/O needed.
    fn inline_vocab() -> std::collections::HashMap<char, usize> {
        [('h', 50usize), ('z', 68), ('\u{00f0}', 81), (' ', 16)]
            .into_iter()
            .collect()
    }

    #[test]
    fn tokenize_known_chars() {
        let vocab = inline_vocab();
        let ids = tokenize("h", &vocab);
        assert_eq!(ids, vec![50]);
    }

    #[test]
    fn tokenize_skips_unknown() {
        let vocab = inline_vocab();
        let ids = tokenize("h\x00z", &vocab); // \x00 not in vocab
        assert_eq!(ids, vec![50, 68]);
    }

    #[test]
    fn tokenize_th_sound() {
        // ð was missing from the old hardcoded vocab
        let vocab = inline_vocab();
        let ids = tokenize("\u{00f0}", &vocab);
        assert_eq!(ids, vec![81]);
    }

    /// Reads the real tokenizer.json; only runs when $KOKORO_MODEL_DIR is set.
    #[test]
    fn build_vocab_from_real_file() {
        let model_dir = match std::env::var("KOKORO_MODEL_DIR") {
            Ok(d) => std::path::PathBuf::from(d),
            Err(_) => {
                eprintln!("Skipping: $KOKORO_MODEL_DIR not set");
                return;
            }
        };
        let vocab = build_vocab(&model_dir).expect("build_vocab failed");
        assert_eq!(vocab.get(&'\u{00f0}'), Some(&81));
        assert_eq!(vocab.get(&' '),         Some(&16));
        assert_eq!(vocab.get(&'.'),         Some(&4));
        assert_eq!(vocab.get(&'\u{02c8}'), Some(&156));
        assert_eq!(vocab.get(&'\u{0254}'), Some(&76));
    }

    #[test]
    fn sentence_timestamp_basic() {
        // BOS=10, two phonemes of 20 each, EOS=5
        // start = 0.0 + 10*2/80 = 0.25s
        // end   = 0.25 + (20+20)*2/80 = 0.25 + 1.0 = 1.25s
        let durations = vec![10.0f32, 20.0, 20.0, 5.0];
        let ts = sentence_timestamp_from_durations("hello", &durations, 0.0);
        assert!((ts.start_sec - 0.25).abs() < 0.001, "start={}", ts.start_sec);
        assert!((ts.end_sec - 1.25).abs() < 0.001, "end={}", ts.end_sec);
    }

    #[test]
    fn sentence_timestamp_with_offset() {
        // Same as above but with a 1.0s time offset
        let durations = vec![10.0f32, 20.0, 20.0, 5.0];
        let ts = sentence_timestamp_from_durations("hello", &durations, 1.0);
        assert!((ts.start_sec - 1.25).abs() < 0.001, "start={}", ts.start_sec);
        assert!((ts.end_sec - 2.25).abs() < 0.001, "end={}", ts.end_sec);
    }

    #[test]
    fn sentence_timestamp_too_short_returns_offset() {
        let ts = sentence_timestamp_from_durations("x", &[1.0, 2.0], 0.5);
        assert_eq!(ts.start_sec, 0.5);
        assert_eq!(ts.end_sec, 0.5);
    }
}
