//! TTS engine — public API and shared synthesis loop.

use std::sync::Arc;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::Stream;
use tokio::sync::mpsc;

use crate::backend::Backend;
use crate::chunk::{AudioSegment, Segment};
use crate::error::TtsError;
use crate::splitter;
use crate::audio_cache;

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
        let text = text.to_string();

        tokio::spawn(async move {
            run_synthesis_loop(text, backend, voice, tx).await;
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

        let paragraph_end = sentence.paragraph_end;
        let sentence_text = sentence.text.clone();
        let backend2 = Arc::clone(&backend);
        let voice2 = voice.clone();

        // Check disk cache for F5 (slow backend only).
        // Kokoro is fast enough that caching adds more complexity than it saves.
        let cache_key = match &voice {
            crate::backend::Voice::F5Tts { .. } => {
                let normalized = crate::f5::normalizer::normalize(&sentence_text);
                Some(audio_cache::cache_key(&normalized, &voice.cache_key()))
            }
            _ => None,
        };

        if let Some(ref key) = cache_key {
            if let Some((samples, sample_rate, duration)) = audio_cache::lookup(key) {
                eprintln!("[audio cache] hit sentence {index}, skipping synthesis");
                let segment = Segment {
                    start: time_offset,
                    end: time_offset + duration,
                    text: sentence.text.clone(),
                };
                time_offset += duration;
                if index + 1 < sentence_count {
                    time_offset += if paragraph_end { paragraph_silence_secs } else { sentence_silence_secs };
                }
                let seg = AudioSegment { index, samples, sample_rate, transcript: segment, paragraph_end };
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
                // Write to disk cache if applicable
                if let Some(ref key) = cache_key {
                    let normalized = crate::f5::normalizer::normalize(&sentence.text);
                    audio_cache::store(key, &normalized, &samples, sample_rate, duration);
                }

                let segment = Segment {
                    start: time_offset,
                    end: time_offset + duration,
                    text: sentence.text.clone(),
                };

                time_offset += duration;
                if index + 1 < sentence_count {
                    time_offset += if paragraph_end {
                        paragraph_silence_secs
                    } else {
                        sentence_silence_secs
                    };
                }

                let seg = AudioSegment { index, samples, sample_rate, transcript: segment, paragraph_end };
                if tx.send(Ok(seg)).await.is_err() { return; }
            }
            Err(e) => {
                eprintln!("Warning: synthesis failed on sentence {index}: {e}");
                if tx.send(Err(e)).await.is_err() { return; }
            }
        }
    }
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
        })
    }
}
