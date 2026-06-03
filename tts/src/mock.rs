//! Mock TTS backend — sine-wave tone, no model weights needed.

use crate::engine::TtsBackend;
use crate::error::TtsError;

pub struct MockBackend;

impl TtsBackend for MockBackend {
    fn synthesize_sentence(
        &self,
        text: &str,
        _voice: &crate::backend::Voice,
        _index: usize,
    ) -> Result<(Vec<f32>, u32, f64), TtsError> {
        let sample_rate = 24_000u32;
        let duration = (text.split_whitespace().count() as f64 * 0.35).max(0.3);
        let n = (sample_rate as f64 * duration) as usize;
        let samples: Vec<f32> = (0..n)
            .map(|i| {
                0.3 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / sample_rate as f32).sin()
            })
            .collect();
        Ok((samples, sample_rate, duration))
    }
}
