//! Kokoro TTS backend — ONNX inference (Rust) + G2P (Python/misaki).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{bail, Result};
use hound::{SampleFormat, WavSpec, WavWriter};
use futures::StreamExt;
use ort::{inputs, session::Session, value::Tensor};

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
        _voice: &crate::backend::Voice,
        index: usize,
    ) -> Result<(Vec<f32>, u32, f64), TtsError> {
        let mut inference = self.inference.lock().map_err(|e| TtsError::SynthesisFailed {
            index,
            sentence: text.to_string(),
            cause: format!("lock poisoned: {e}"),
        })?;
        inference.synthesize(text, index).map_err(|e| TtsError::SynthesisFailed {
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
    voice: String,
    speed: f32,
    vocab: std::collections::HashMap<char, usize>,
    g2p: G2pEngine,
}

impl KokoroInference {
    fn new(model_dir: PathBuf, voice: &str, speed: f32, g2p: G2pEngine) -> Result<Self> {
        let model_path = model_dir.join("model.onnx");
        if !model_path.exists() {
            bail!("model.onnx not found in {}.\nRun setup.sh to download it.", model_dir.display());
        }

        eprintln!("Loading Kokoro ONNX model…");
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
        eprintln!("Kokoro model ready.");

        let vocab = build_vocab(&model_dir)?;

        Ok(Self { session, model_dir, voice: voice.to_string(), speed, vocab, g2p })
    }

    fn synthesize(&mut self, text: &str, _index: usize) -> Result<(Vec<f32>, u32, f64)> {
        let rt = tokio::runtime::Runtime::new()?;
        let phonemes = rt.block_on(async {
            let mut stream = self.g2p.phonemize(text);
            let mut result = String::new();
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(c) => result = c.phonemes,
                    Err(e) => eprintln!("Warning: G2P error: {e}"),
                }
            }
            result
        });

        if phonemes.is_empty() {
            bail!("No phonemes produced for: {text:?}");
        }

        let token_ids = tokenize(&phonemes, &self.vocab);
        if token_ids.is_empty() {
            bail!("No token IDs for phonemes: {phonemes:?}");
        }

        let (samples, durations) = self.run_onnx(&token_ids)?;
        let duration = audio_duration_from_durations(&durations);

        Ok((samples, 24_000, duration))
    }

    fn run_onnx(&mut self, token_ids: &[i64]) -> Result<(Vec<f32>, Vec<f32>)> {
        let n_tokens = token_ids.len();
        let style = load_voice_style(&self.model_dir, &self.voice, n_tokens)?;

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
            Err(_) => { eprintln!("Skipping: $KOKORO_MODEL_DIR not set"); return; }
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
}
