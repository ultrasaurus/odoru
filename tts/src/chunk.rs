#[derive(Debug, Clone)]
pub struct Segment {
    /// Sentence start time in seconds, relative to the full synthesis output.
    pub start: f64,
    /// Sentence end time in seconds.
    pub end: f64,
    /// The sentence text.
    pub text: String,
}

/// One synthesized sentence: audio samples + transcript segment with timing.
pub struct AudioSegment {
    /// Zero-based position of this sentence in the stream.
    pub index: usize,
    /// Raw f32 PCM samples at `sample_rate` Hz, mono.
    pub samples: Vec<f32>,
    /// Sample rate in Hz (typically 24 000).
    pub sample_rate: u32,
    /// Sentence text and timing.
    pub transcript: Segment,
    /// True if this is the last sentence in its paragraph.
    pub paragraph_end: bool,
}
