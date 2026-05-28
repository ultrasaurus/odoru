//! # tts
//!
//! Streaming TTS API — text → per-sentence `AudioSegment` stream.
//!
//! ## Example
//!
//! ```no_run
//! use ko_odoru::tts::Tts;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let tts = Tts::builder()
//!         .voice("af_heart")
//!         .speed(1.0)
//!         .build()?;
//!
//!     let mut stream = tts.synthesize("Hello world. The cat sat on the mat.");
//!     while let Some(result) = stream.next().await {
//!         let seg = result?;
//!         println!("[{:.3}s – {:.3}s] {}",
//!             seg.transcript.start,
//!             seg.transcript.end,
//!             seg.transcript.text);
//!     }
//!     Ok(())
//! }
//! ```

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Result};
use futures::StreamExt;
use hound::{SampleFormat, WavSpec, WavWriter};
use ort::{inputs, session::Session, value::Tensor};
use tokio::sync::Mutex;

use crate::engine::G2pEngine;
use crate::transcript::Segment;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// One synthesized sentence: audio samples + transcript segment with timing.
pub struct AudioSegment {
    /// Raw f32 audio samples at `sample_rate` Hz, mono.
    pub samples: Vec<f32>,
    /// Always 24000.
    pub sample_rate: u32,
    /// Sentence text and timing. `words` is empty until word-level alignment
    /// is implemented — see `single_segment_start_end_match_words` test.
    pub transcript: Segment,
}

impl AudioSegment {
    /// Write this segment's audio to a 16-bit PCM WAV file.
    pub fn save_wav(&self, path: impl AsRef<std::path::Path>) -> Result<()> {
        let path = path.as_ref();
        let spec = WavSpec {
            channels: 1,
            sample_rate: self.sample_rate,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut writer = WavWriter::create(path, spec)
            .map_err(|e| anyhow::anyhow!("Create WAV {}: {e}", path.display()))?;
        for &s in &self.samples {
            let pcm = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
            writer.write_sample(pcm)
                .map_err(|e| anyhow::anyhow!("WAV write: {e}"))?;
        }
        writer.finalize()
            .map_err(|e| anyhow::anyhow!("WAV finalize: {e}"))
    }
}

/// Save a slice of `AudioSegment`s as a single concatenated WAV file,
/// with `gap_ms` milliseconds of silence between segments.
pub fn save_wav_all(
    segments: &[AudioSegment],
    path: impl AsRef<std::path::Path>,
    gap_ms: u32,
) -> Result<()> {
    let path = path.as_ref();
    if segments.is_empty() {
        bail!("No segments to write");
    }
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
            writer.write_sample(pcm)
                .map_err(|e| anyhow::anyhow!("WAV write: {e}"))?;
        }
        if i + 1 < segments.len() {
            for _ in 0..silence_len {
                writer.write_sample(0i16)
                    .map_err(|e| anyhow::anyhow!("WAV silence: {e}"))?;
            }
        }
    }
    writer.finalize()
        .map_err(|e| anyhow::anyhow!("WAV finalize: {e}"))
}

// ---------------------------------------------------------------------------
// TtsBuilder
// ---------------------------------------------------------------------------

/// Builder for `Tts`. Obtain via `Tts::builder()` or `Tts::default()`.
pub struct TtsBuilder {
    voice: String,
    speed: f32,
    model_dir: PathBuf,
    venv_path: Option<PathBuf>,
}

impl TtsBuilder {
    fn new() -> Self {
        let model_dir = std::env::var("KOKORO_MODEL_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
                PathBuf::from(home).join(".kokoro")
            });
        Self {
            voice: "am_puck".into(),
            speed: 1.0,
            model_dir,
            venv_path: None,
        }
    }

    /// Set the voice (e.g. `"am_puck"`, `"af_heart"`).
    pub fn voice(mut self, voice: &str) -> Self {
        self.voice = voice.into();
        self
    }

    /// Set the speed multiplier (1.0 = normal).
    pub fn speed(mut self, speed: f32) -> Self {
        self.speed = speed;
        self
    }

    /// Override the model directory (default: `$KOKORO_MODEL_DIR` or `~/.kokoro`).
    pub fn model_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.model_dir = path.into();
        self
    }

    /// Override the misaki-g2p venv path (default: `$MISAKI_VENV`).
    pub fn venv_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.venv_path = Some(path.into());
        self
    }

    /// Build the `Tts` engine. Fails early if the model or venv can't be loaded.
    pub fn build(self) -> Result<Tts> {
        let model_path = self.model_dir.join("model.onnx");
        if !model_path.exists() {
            bail!(
                "model.onnx not found in {}.\nRun setup.sh to download it.",
                self.model_dir.display()
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

        let venv = self.venv_path.as_deref();
        let g2p = G2pEngine::new(venv)
            .map_err(|e| anyhow::anyhow!("G2P engine: {e}"))?;

        let vocab = crate::synth::build_vocab(&self.model_dir)?;

        Ok(Tts {
            inner: Arc::new(Mutex::new(TtsInner { session, g2p })),
            config: Arc::new(TtsConfig {
                voice: self.voice,
                speed: self.speed,
                model_dir: self.model_dir,
                vocab,
            }),
        })
    }
}

// ---------------------------------------------------------------------------
// Tts
// ---------------------------------------------------------------------------

struct TtsConfig {
    voice: String,
    speed: f32,
    model_dir: PathBuf,
    vocab: std::collections::HashMap<char, usize>,
}

struct TtsInner {
    session: Session,
    g2p: G2pEngine,
}

/// The TTS engine. Cheap to clone — inner state is `Arc`-wrapped.
pub struct Tts {
    inner: Arc<Mutex<TtsInner>>,
    config: Arc<TtsConfig>,
}

// Manual Clone since TtsConfig doesn't need to be Clone publicly
impl Clone for Tts {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            config: Arc::clone(&self.config),
        }
    }
}

impl Tts {
    /// Returns a `TtsBuilder` with default settings.
    pub fn builder() -> TtsBuilder {
        TtsBuilder::new()
    }

    /// Synthesize `text`, streaming one `AudioSegment` per sentence.
    pub fn synthesize(&self, text: &str) -> TtsStream {
        TtsStream::new(text.to_string(), Arc::clone(&self.inner), Arc::clone(&self.config))
    }
}

impl Default for Tts {
    fn default() -> Self {
        TtsBuilder::new().build().expect("Failed to build default Tts")
    }
}

// ---------------------------------------------------------------------------
// TtsStream
// ---------------------------------------------------------------------------

/// Stream of `AudioSegment`s — one per sentence in the input text.
/// Drive with `while let Some(result) = stream.next().await { ... }`.
pub struct TtsStream {
    inner: std::pin::Pin<Box<dyn futures::Stream<Item = Result<AudioSegment>> + Send>>,
}

impl TtsStream {
    fn new(text: String, inner: Arc<Mutex<TtsInner>>, config: Arc<TtsConfig>) -> Self {
        let stream = async_stream::try_stream! {
            // Phonemize all sentences up front
            let phoneme_chunks = {
                let guard = inner.lock().await;
                    let mut stream = guard.g2p.phonemize(&text);
                let mut chunks: Vec<(String, String)> = Vec::new();
                while let Some(result) = stream.next().await {
                    match result {
                        Ok(chunk) => chunks.push((chunk.sentence, chunk.phonemes)),
                        Err(e) => eprintln!("Warning: G2P failed: {e}"),
                    }
                }
                chunks
            };

            if phoneme_chunks.is_empty() {
                Err(anyhow::anyhow!("No phonemes produced from input text"))?;
            }

            let mut time_offset = 0.0f32;
            let silence_samples = 2400usize; // 100ms @ 24kHz
            let silence_secs = silence_samples as f32 / 24_000.0;

            for (i, (sentence, phonemes)) in phoneme_chunks.iter().enumerate() {
                let token_ids = crate::synth::tokenize(phonemes, &config.vocab);
                if token_ids.is_empty() {
                    eprintln!("Warning: no tokens for sentence {i}, skipping");
                    continue;
                }

                let (samples, durations) = {
                    let mut guard = inner.lock().await;
                    run_inference(&mut guard.session, &token_ids, &config, &config.model_dir)?
                };

                let segment = sentence_to_segment(sentence, &durations, time_offset);
                let chunk_secs = samples.len() as f32 / 24_000.0;
                time_offset += chunk_secs;
                if i + 1 < phoneme_chunks.len() {
                    time_offset += silence_secs;
                }

                yield AudioSegment {
                    samples,
                    sample_rate: 24_000,
                    transcript: segment,
                };
            }
        };

        Self { inner: Box::pin(stream) }
    }

    /// Advance the stream, returning the next `AudioSegment` or `None` if done.
    pub async fn next(&mut self) -> Option<Result<AudioSegment>> {
        self.inner.next().await
    }
}

// ---------------------------------------------------------------------------
// Inference
// ---------------------------------------------------------------------------

fn run_inference(
    session: &mut Session,
    token_ids: &[i64],
    config: &TtsConfig,
    model_dir: &std::path::Path,
) -> Result<(Vec<f32>, Vec<f32>)> {
    let n_tokens = token_ids.len();
    let style = load_voice(model_dir, &config.voice, n_tokens)?;

    let mut ids = Vec::with_capacity(n_tokens + 2);
    ids.push(0i64); // BOS
    ids.extend_from_slice(token_ids);
    ids.push(0i64); // EOS
    let seq_len = ids.len();

    let input_ids_tensor =
        Tensor::<i64>::from_array(([1, seq_len], ids.into_boxed_slice()))
            .map_err(|e| anyhow::anyhow!("input_ids: {e}"))?;
    let style_tensor =
        Tensor::<f32>::from_array(([1usize, 256], style.into_boxed_slice()))
            .map_err(|e| anyhow::anyhow!("style: {e}"))?;
    let speed_tensor =
        Tensor::<f32>::from_array(([1usize], vec![config.speed].into_boxed_slice()))
            .map_err(|e| anyhow::anyhow!("speed: {e}"))?;

    let outputs = session
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

fn load_voice(model_dir: &std::path::Path, voice: &str, n_tokens: usize) -> Result<Vec<f32>> {
    let path = model_dir.join("voices").join(format!("{}.bin", voice));
    if !path.exists() {
        bail!("Voice file not found: {}", path.display());
    }
    let bytes = std::fs::read(&path)
        .map_err(|e| anyhow::anyhow!("Read voice: {e}"))?;
    let floats: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    let offset = n_tokens * 256;
    if offset + 256 > floats.len() {
        bail!("Voice file too short for {n_tokens} tokens");
    }
    Ok(floats[offset..offset + 256].to_vec())
}

// ---------------------------------------------------------------------------
// Timestamp mapping
// ---------------------------------------------------------------------------

fn sentence_to_segment(sentence: &str, durations: &[f32], time_offset: f32) -> Segment {
    let half_frames_to_secs = |hf: f32| (hf * 2.0 / 80.0) as f64;

    let (start, end) = if durations.len() >= 3 {
        let start = time_offset as f64 + half_frames_to_secs(durations[0]);
        let phoneme_dur: f32 = durations[1..durations.len() - 1].iter().sum();
        let end = start + half_frames_to_secs(phoneme_dur);
        (round3(start), round3(end))
    } else {
        let t = time_offset as f64;
        (t, t)
    };

    Segment {
        start,
        end,
        text: sentence.to_string(),
        words: Vec::new(), // populated when word-level alignment is added
        speaker: None,
    }
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Collect all segments from a stream into a Vec.
    async fn collect(mut stream: TtsStream) -> Vec<AudioSegment> {
        let mut out = Vec::new();
        while let Some(result) = stream.next().await {
            out.push(result.expect("synthesis failed"));
        }
        out
    }

    /// Build a Tts for testing, skipping if env vars aren't set.
    /// Returns None if MISAKI_VENV or KOKORO_MODEL_DIR is unavailable.
    fn try_build_tts() -> Option<Tts> {
        if std::env::var("MISAKI_VENV").is_err() {
            eprintln!("Skipping: $MISAKI_VENV not set");
            return None;
        }
        Some(Tts::builder().build().expect("Tts::build failed"))
    }

    #[tokio::test]
    async fn streams_segments_with_timestamps() {
        let Some(tts) = try_build_tts() else { return; };
        let mut stream = tts.synthesize("Hello world. The quick brown fox jumps.");

        let mut segments = vec![];
        while let Some(result) = stream.next().await {
            let seg = result.expect("synthesis failed");
            segments.push(seg);
        }

        assert!(segments.len() >= 2);

        let first = &segments[0];
        assert!(!first.samples.is_empty());
        assert!(first.sample_rate > 0);

        // Word end times should not exceed audio duration (with 100ms tolerance)
        let audio_duration = first.samples.len() as f64 / first.sample_rate as f64;
        let last_word_end = first.transcript.words
            .iter()
            .filter_map(|w| w.end)
            .fold(0.0_f64, f64::max);
        assert!(last_word_end <= audio_duration + 0.1);

        // Word start times should be monotonically increasing
        let starts: Vec<f64> = first.transcript.words
            .iter()
            .filter_map(|w| w.start)
            .collect();
        assert!(starts.windows(2).all(|w| w[0] <= w[1]));
    }

    #[tokio::test]
    async fn single_sentence_yields_one_segment() {
        let Some(tts) = try_build_tts() else { return; };
        let segments = collect(tts.synthesize("Hello world.")).await;
        assert_eq!(segments.len(), 1);
        assert!(!segments[0].samples.is_empty());
        assert_eq!(segments[0].sample_rate, 24_000);
    }

    #[tokio::test]
    async fn segment_timestamps_are_monotonic() {
        let Some(tts) = try_build_tts() else { return; };
        let segments = collect(
            tts.synthesize("Hello world. The cat sat on the mat. How are you?")
        ).await;
        assert_eq!(segments.len(), 3);
        // Each segment should start after the previous one ends
        for w in segments.windows(2) {
            assert!(w[1].transcript.start >= w[0].transcript.end,
                "segment {} start {:.3} < segment {} end {:.3}",
                1, w[1].transcript.start, 0, w[0].transcript.end);
        }
    }

    /// Word-level timestamps — ignored until word alignment is implemented.
    #[tokio::test]
    #[ignore]
    async fn single_segment_start_end_match_words() {
        let Some(tts) = try_build_tts() else { return; };
        let mut stream = tts.synthesize("Hello world.");

        let seg = stream.next().await
            .expect("expected a segment")
            .expect("synthesis failed");

        assert!(stream.next().await.is_none());

        let words = &seg.transcript.words;
        assert!(!words.is_empty());

        let first_word_start = words.iter()
            .filter_map(|w| w.start)
            .next()
            .expect("first word should have a start time");

        let last_word_end = words.iter()
            .filter_map(|w| w.end)
            .last()
            .expect("last word should have an end time");

        assert_eq!(seg.transcript.start, first_word_start);
        assert_eq!(seg.transcript.end, last_word_end);
    }
}
