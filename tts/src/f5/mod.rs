//! F5-TTS backend — MLX inference via Python bridge.

pub mod normalizer;

use std::sync::Arc;

use pyo3::ffi::c_str;
use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::backend::Voice;
use crate::engine::TtsBackend;
use crate::error::TtsError;

pub struct F5Backend {
    module: Arc<Py<PyAny>>,
    voices: Vec<Voice>,
}

// Py<PyAny> is not Send by default; we only access it inside spawn_blocking
// while holding the GIL, satisfying PyO3's safety requirements.
unsafe impl Send for F5Backend {}
unsafe impl Sync for F5Backend {}

impl F5Backend {
    pub fn init(voices: Vec<Voice>, _workers: usize) -> Result<Self, anyhow::Error> {
        if voices.is_empty() {
            anyhow::bail!("F5Backend requires at least one voice");
        }

        let module = Python::attach(|py| -> PyResult<Py<PyAny>> {
            crate::python::bridge::load_module(
                py,
                c_str!(include_str!("../tts.py")),
                c"tts.py",
                c"tts",
            )
        })
        .map_err(|e| anyhow::anyhow!("Failed to load tts.py: {e}"))?;

        Ok(Self { module: Arc::new(module), voices })
    }
}

impl TtsBackend for F5Backend {
    fn synthesize_sentence(
        &self,
        text: &str,
        index: usize,
    ) -> Result<(Vec<f32>, u32, f64), TtsError> {
        let text = normalizer::normalize(text);
        let module = Arc::clone(&self.module);

        let voice = self.voices.first()
            .ok_or_else(|| TtsError::UnknownVoice("no voices configured".into()))?;

        let (voice_ref, ref_text, speed, cfg_strength) = match voice {
            Voice::F5Tts { voice_ref, ref_text, speed, cfg_strength, .. } => (
                voice_ref.to_string_lossy().into_owned(),
                ref_text.clone(),
                *speed,
                *cfg_strength,
            ),
            _ => return Err(TtsError::UnknownVoice("expected F5Tts voice".into())),
        };

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
