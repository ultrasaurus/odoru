//! TTS engine — public API and shared synthesis loop.

use tracing::{debug, warn};
use std::sync::Arc;
use std::pin::Pin;
use std::task::{Context, Poll};

use dashmap::DashMap;
use futures::Stream;
use tokio::sync::{mpsc, Mutex};

use crate::backend::Backend;
use crate::chunk::{AudioSegment, Segment};
use crate::error::TtsError;
use crate::splitter;
use crate::audio_cache;

/// Per-sentence synthesis lock. Keyed by audio cache key (SHA-256 of
/// normalised text + voice cache key). Prevents two concurrent callers
/// (e.g. a WS session and a background job) from synthesising the same
/// sentence simultaneously — the second waits, then gets a disk cache hit.
type SynthLocks = Arc<DashMap<String, Arc<Mutex<()>>>>;

// ---------------------------------------------------------------------------
// TtsBackend trait
// ---------------------------------------------------------------------------

/// Blocking per-sentence synthesis. Implement this for each backend.
/// Called inside `spawn_blocking` — never blocks the async executor.
/// Returns `(samples, sample_rate, duration_secs)`.
pub trait TtsBackend: Send + Sync {
    fn synthesize_sentence(
        &self,
        text: &str,
        voice: &crate::backend::Voice,
        index: usize,
    ) -> Result<(Vec<f32>, u32, f64), TtsError>;
}

// ---------------------------------------------------------------------------
// TtsEngine
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct TtsEngine {
    backend: Arc<dyn TtsBackend>,
    voices: Arc<std::collections::HashMap<String, crate::backend::Voice>>,
    synth_locks: SynthLocks,
}

impl TtsEngine {
    pub fn builder() -> TtsEngineBuilder {
        TtsEngineBuilder::default()
    }

    /// List available voice names.
    pub fn voice_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.voices.keys().cloned().collect();
        names.sort();
        names
    }

    /// Return the cache key string for a named voice, or `None` if unknown.
    pub fn voice_cache_key(&self, voice_name: &str) -> Option<String> {
        self.voices.get(voice_name).map(|v| v.cache_key())
    }

    /// Check whether every sentence in `text` is already in the audio disk cache
    /// for the given voice. Returns `None` if the voice name is not found.
    pub fn all_audio_cached(&self, text: &str, voice_name: &str) -> Option<bool> {
        let voice = self.voices.get(voice_name)?.clone();
        let sentences: Vec<_> = splitter::split(text)
            .into_iter()
            .filter(|s| !s.text.trim().is_empty())
            .collect();
        if sentences.is_empty() {
            return Some(false);
        }
        let all_cached = sentences.iter()
            .all(|s| {
                match &voice {
                    crate::backend::Voice::F5Tts { .. } => {
                        let normalized = crate::f5::normalizer::normalize(&s.text);
                        let key = audio_cache::cache_key(&normalized, &voice.cache_key());
                        audio_cache::exists(&key)
                    }
                    crate::backend::Voice::Kokoro { .. } => {
                        let key = audio_cache::cache_key(&s.text, &voice.cache_key());
                        audio_cache::exists(&key)
                    }
                    _ => false,
                }
            });
        Some(all_cached)
    }

    /// Synthesise `text` using the named voice, streaming one `AudioSegment` per sentence.
    /// Returns an error stream if `voice_name` is not found.
    pub fn synthesize(&self, text: &str, voice_name: &str) -> AudioStream {
        let voice = match self.voices.get(voice_name) {
            Some(v) => v.clone(),
            None => {
                let (tx, rx) = mpsc::channel(1);
                let err = TtsError::UnknownVoice(voice_name.to_string());
                tokio::spawn(async move { let _ = tx.send(Err(err)).await; });
                return AudioStream { rx };
            }
        };

        let (tx, rx) = mpsc::channel(4);
        let backend = Arc::clone(&self.backend);
        let synth_locks = Arc::clone(&self.synth_locks);
        let text = text.to_string();

        tokio::spawn(async move {
            run_synthesis_loop(text, backend, voice, synth_locks, tx).await;
        });

        AudioStream { rx }
    }
}

// ---------------------------------------------------------------------------
// Shared synthesis loop
// ---------------------------------------------------------------------------

async fn run_synthesis_loop(
    text: String,
    backend: Arc<dyn TtsBackend>,
    voice: crate::backend::Voice,
    synth_locks: SynthLocks,
    tx: mpsc::Sender<Result<AudioSegment, TtsError>>,
) {
    let sentences = splitter::split(&text);
    let mut time_offset: f64 = 0.0;
    let sentence_silence_secs = 0.1;
    let paragraph_silence_secs = 0.5;
    let sentence_count = sentences.len();

    for (index, sentence) in sentences.into_iter().enumerate() {
        if sentence.text.trim().is_empty() {
            continue;
        }
        if sentence.text.chars().filter(|c| c.is_alphabetic()).count() == 0 {
            continue;
        }

        let paragraph_end = sentence.paragraph_end;
        let sentence_text = sentence.text.clone();
        let backend2 = Arc::clone(&backend);
        let voice2 = voice.clone();

        // Compute (disk_cache_key, text_for_metadata) for backends that support
        // disk caching. Use a per-sentence lock to prevent duplicate synthesis
        // when a WS session and a background job race to the same sentence.
        // Pattern: acquire lock → re-check disk cache → synthesize if still a
        // miss → write → release. Second caller waits, then gets a cache hit.
        let cache_entry: Option<(String, String)> = match &voice {
            crate::backend::Voice::F5Tts { .. } => {
                let normalized = crate::f5::normalizer::normalize(&sentence_text);
                let key = audio_cache::cache_key(&normalized, &voice.cache_key());
                Some((key, normalized))
            }
            crate::backend::Voice::Kokoro { .. } => {
                let key = audio_cache::cache_key(&sentence_text, &voice.cache_key());
                Some((key, sentence_text.clone()))
            }
            _ => None,
        };

        // Short-sentence guard: F5 takes 1-2 minutes on inputs with very few
        // alphabetic characters (e.g. "I.", "A." from outline headers after splitting).
        // Emit a short silence instead of synthesizing.
        if let Some((_, ref normalized)) = cache_entry {
            if matches!(&voice, crate::backend::Voice::F5Tts { .. }) {
                let alpha_count = normalized.chars().filter(|c| c.is_alphabetic()).count();
                if alpha_count < 3 {
                    let silence_duration = sentence_silence_secs;
                    let silence_samples = vec![0f32; (24_000.0 * silence_duration) as usize];
                    let mp3_bytes = audio_cache::encode_mp3(&silence_samples, 24_000);
                    let seg = make_segment(index, mp3_bytes, silence_duration, &sentence, time_offset);
                    time_offset = advance_offset(time_offset, silence_duration, index, sentence_count, paragraph_end, sentence_silence_secs, paragraph_silence_secs);
                    debug!("[engine] skipping short F5 sentence {index}: {:?} ({alpha_count} alpha chars)", normalized);
                    if tx.send(Ok(seg)).await.is_err() { return; }
                    continue;
                }
            }
        }

        if let Some((ref key, _)) = cache_entry {
            // Fast path: check before acquiring the lock.
            let key2 = key.clone();
            let hit = tokio::task::spawn_blocking(move || audio_cache::lookup(&key2))
                .await.expect("spawn_blocking panicked");
            if let Some((mp3_bytes, duration)) = hit {
                debug!("[audio cache] hit sentence {index} (pre-lock), skipping synthesis");
                let seg = make_segment(index, mp3_bytes, duration, &sentence, time_offset);
                time_offset = advance_offset(time_offset, duration, index, sentence_count, paragraph_end, sentence_silence_secs, paragraph_silence_secs);
                if tx.send(Ok(seg)).await.is_err() { return; }
                continue;
            }

            // Acquire per-sentence lock, then re-check (another synthesiser may
            // have finished while we waited).
            let lock = synth_locks
                .entry(key.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone();
            let _guard = lock.lock().await;

            let key2 = key.clone();
            let hit = tokio::task::spawn_blocking(move || audio_cache::lookup(&key2))
                .await.expect("spawn_blocking panicked");
            if let Some((mp3_bytes, duration)) = hit {
                debug!("[audio cache] hit sentence {index} (post-lock), skipping synthesis");
                let seg = make_segment(index, mp3_bytes, duration, &sentence, time_offset);
                time_offset = advance_offset(time_offset, duration, index, sentence_count, paragraph_end, sentence_silence_secs, paragraph_silence_secs);
                if tx.send(Ok(seg)).await.is_err() { return; }
                continue;
            }
        }

        let result = tokio::task::spawn_blocking(move || {
            backend2.synthesize_sentence(&sentence_text, &voice2, index)
        })
        .await
        .expect("spawn_blocking panicked");

        match result {
            Ok((samples, sample_rate, duration)) => {
                // Encode to MP3 and write to disk cache (lock still held until end of match arm).
                let mp3_bytes = tokio::task::spawn_blocking({
                    let samples2 = samples.clone();
                    move || audio_cache::encode_mp3(&samples2, sample_rate)
                }).await.expect("spawn_blocking panicked");

                if let Some((ref key, ref text)) = cache_entry {
                    let (key2, text2, mp3_2) = (key.clone(), text.clone(), mp3_bytes.clone());
                    tokio::task::spawn_blocking(move || {
                        audio_cache::store(&key2, &text2, &mp3_2, duration);
                    }).await.expect("spawn_blocking panicked");
                }

                let seg = make_segment(index, mp3_bytes, duration, &sentence, time_offset);
                time_offset = advance_offset(time_offset, duration, index, sentence_count, paragraph_end, sentence_silence_secs, paragraph_silence_secs);
                if tx.send(Ok(seg)).await.is_err() { return; }
            }
            Err(e) => {
                warn!("Synthesis failed on sentence {index}: {e}");
                if tx.send(Err(e)).await.is_err() { return; }
            }
        }
    }
}

fn make_segment(
    index: usize,
    mp3_bytes: Vec<u8>,
    duration: f64,
    sentence: &crate::splitter::Sentence,
    time_offset: f64,
) -> AudioSegment {
    let transcript = Segment {
        start: time_offset,
        end: time_offset + duration,
        text: sentence.text.clone(),
        words: vec![],
        speaker: None,
    };
    AudioSegment { index, audio: mp3_bytes, duration, transcript, paragraph_end: sentence.paragraph_end }
}

/// Advance a cumulative timeline offset past one sentence's audio plus the
/// inter-sentence/paragraph pause that follows it (no pause after the last
/// sentence). `pub` so a replay-only synthesis path (e.g. for imported,
/// non-live-synthesizable voices) can reproduce identical transcript timing
/// without duplicating these silence constants.
pub fn advance_offset(
    offset: f64,
    duration: f64,
    index: usize,
    sentence_count: usize,
    paragraph_end: bool,
    sentence_silence_secs: f64,
    paragraph_silence_secs: f64,
) -> f64 {
    let mut t = offset + duration;
    if index + 1 < sentence_count {
        t += if paragraph_end { paragraph_silence_secs } else { sentence_silence_secs };
    }
    t
}

// ---------------------------------------------------------------------------
// AudioStream
// ---------------------------------------------------------------------------

pub struct AudioStream {
    rx: mpsc::Receiver<Result<AudioSegment, TtsError>>,
}

impl Stream for AudioStream {
    type Item = Result<AudioSegment, TtsError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

// ---------------------------------------------------------------------------
// TtsEngineBuilder
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct TtsEngineBuilder {
    backend: Option<Backend>,
}

impl TtsEngineBuilder {
    pub fn backend(mut self, backend: Backend) -> Self {
        self.backend = Some(backend);
        self
    }

    pub fn build(self) -> Result<TtsEngine, TtsError> {
        let backend_config = self.backend.unwrap_or(Backend::Mock);

        // Step 1: initialize Python + venv FIRST before anything else
        pyo3::Python::attach(|py| {
            crate::python::setup::setup(py)
                .map_err(|e| TtsError::PythonInit(e.to_string()))
        })?;

        // Collect voices from backend config for the engine-level voice registry
        let voices: std::collections::HashMap<String, crate::backend::Voice> =
            match &backend_config {
                Backend::Kokoro { voice, all_voices, .. } => {
                    let mut voices_to_register = all_voices.clone();
                    if voices_to_register.is_empty() {
                        voices_to_register.push(voice.clone());
                    }
                    voices_to_register.iter()
                        .map(|n| (n.clone(), crate::backend::Voice::Kokoro { name: n.clone() }))
                        .collect()
                }
                Backend::F5Tts { voices, .. } => {
                    voices.iter()
                        .map(|v| (v.name().to_string(), v.clone()))
                        .collect()
                }
                Backend::Mock => {
                    let v = crate::backend::Voice::Mock;
                    std::iter::once(("mock".to_string(), v)).collect()
                }
            };

        // Step 2: construct backend (Python already initialized)
        let backend: Arc<dyn TtsBackend> = match backend_config {
            Backend::Kokoro { model_dir, voice, speed, .. } => {
                let g2p = crate::g2p::G2pEngine::new()
                    .map_err(|e| TtsError::PythonInit(e.to_string()))?;
                Arc::new(
                    crate::kokoro::KokoroBackend::init(model_dir, &voice, speed, g2p)
                        .map_err(|e| TtsError::PythonInit(e.to_string()))?,
                )
            }
            Backend::F5Tts { workers, .. } => {
                Arc::new(
                    crate::f5::F5Backend::init(workers)
                        .map_err(|e| TtsError::PythonInit(e.to_string()))?,
                )
            }
            Backend::Mock => Arc::new(crate::mock::MockBackend),
        };

        Ok(TtsEngine {
            backend,
            voices: Arc::new(voices),
            synth_locks: Arc::new(DashMap::new()),
        })
    }
}
