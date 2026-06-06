#[derive(Debug, Clone)]
pub struct Segment {
    /// Sentence start time in seconds, relative to the full synthesis output.
    pub start: f64,
    /// Sentence end time in seconds.
    pub end: f64,
    /// The sentence text.
    pub text: String,
}

/// One synthesized sentence: MP3-encoded audio + transcript segment with timing.
pub struct AudioSegment {
    /// Zero-based position of this sentence in the stream.
    pub index: usize,
    /// MP3-encoded audio bytes.
    pub audio: Vec<u8>,
    /// Duration in seconds.
    pub duration: f64,
    /// Sentence text and timing.
    pub transcript: Segment,
    /// True if this is the last sentence in its paragraph.
    pub paragraph_end: bool,
}
