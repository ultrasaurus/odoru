pub struct AudioConfig {
    pub sample_rate: u32,
    pub paragraph_silence_ms: u32,
    pub heading_silence_ms: u32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            sample_rate: 24_000,
            paragraph_silence_ms: 500,
            heading_silence_ms: 800,
        }
    }
}

pub fn silence_samples(ms: u32, sample_rate: u32) -> Vec<f32> {
    vec![0.0f32; (sample_rate * ms / 1000) as usize]
}