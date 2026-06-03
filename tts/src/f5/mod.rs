//! F5-TTS backend — MLX inference via Python bridge.
//!
//! Workers are voice-agnostic: each holds a loaded Python module copy.
//! Voice parameters (ref audio, speed, cfg_strength) are passed per call.
//! Workers are dispatched round-robin via an atomic counter.

pub mod normalizer;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use pyo3::ffi::c_str;
use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::backend::Voice;
use crate::engine::TtsBackend;
use crate::error::TtsError;

pub struct F5Backend {
    /// One Python module per worker — each holds its own model copy.
    workers: Vec<Arc<Py<PyAny>>>,
    /// Round-robin counter.
    next_worker: AtomicUsize,
}

// Py<PyAny> is not Send by default; we only access it inside spawn_blocking
// while holding the GIL, satisfying PyO3's safety requirements.
unsafe impl Send for F5Backend {}
unsafe impl Sync for F5Backend {}

impl F5Backend {
    pub fn init(worker_count: usize) -> Result<Self, anyhow::Error> {
        let worker_count = worker_count.max(1);
        eprintln!("Loading F5-TTS Python module ({worker_count} worker(s))…");

        let mut workers = Vec::with_capacity(worker_count);
        for i in 0..worker_count {
            let module = Python::attach(|py| -> PyResult<Py<PyAny>> {
                crate::python::bridge::load_module(
                    py,
                    c_str!(include_str!("../tts.py")),
                    c"tts.py",
                    c"tts",
                )
            })
            .map_err(|e| anyhow::anyhow!("Worker {i}: failed to load tts.py: {e}"))?;
            workers.push(Arc::new(module));
        }

        eprintln!("F5-TTS ready ({worker_count} worker(s)).");
        Ok(Self { workers, next_worker: AtomicUsize::new(0) })
    }

    fn pick_worker(&self) -> Arc<Py<PyAny>> {
        let idx = self.next_worker.fetch_add(1, Ordering::Relaxed) % self.workers.len();
        Arc::clone(&self.workers[idx])
    }
}

impl TtsBackend for F5Backend {
    fn synthesize_sentence(
        &self,
        text: &str,
        voice: &Voice,
        index: usize,
    ) -> Result<(Vec<f32>, u32, f64), TtsError> {
        let text = normalizer::normalize(text);

        let (voice_ref, ref_text, speed, cfg_strength) = match voice {
            Voice::F5Tts { voice_ref, ref_text, speed, cfg_strength, .. } => (
                voice_ref.to_string_lossy().into_owned(),
                ref_text.clone(),
                *speed,
                *cfg_strength,
            ),
            _ => return Err(TtsError::UnknownVoice(
                format!("F5Backend received non-F5 voice: {:?}", voice.name())
            )),
        };

        let module = self.pick_worker();

        Python::attach(move |py| {
            let kwargs = PyDict::new(py);
            kwargs.set_item("backend", "f5_tts").map_err(|e| synth_err(index, &text, e))?;
            kwargs.set_item("voice_ref", &voice_ref).map_err(|e| synth_err(index, &text, e))?;
            kwargs.set_item("ref_text", &ref_text).map_err(|e| synth_err(index, &text, e))?;
            kwargs.set_item("speed", speed).map_err(|e| synth_err(index, &text, e))?;
            kwargs.set_item("cfg_strength", cfg_strength).map_err(|e| synth_err(index, &text, e))?;

            let result = module.bind(py)
                .getattr("synthesize_sentence")
                .and_then(|f| f.call((text.as_str(),), Some(&kwargs)))
                .map_err(|e| synth_err(index, &text, e))?;

            let samples = crate::python::bridge::extract_f32_list(&result, "samples")
                .map_err(|e| synth_err(index, &text, e))?;
            let sample_rate = crate::python::bridge::extract_u32(&result, "sample_rate")
                .map_err(|e| synth_err(index, &text, e))?;
            let duration = crate::python::bridge::extract_f64(&result, "duration")
                .map_err(|e| synth_err(index, &text, e))?;

            Ok((samples, sample_rate, duration))
        })
    }
}

fn synth_err(index: usize, sentence: &str, e: impl std::fmt::Display) -> TtsError {
    TtsError::SynthesisFailed {
        index,
        sentence: sentence.to_owned(),
        cause: e.to_string(),
    }
}
