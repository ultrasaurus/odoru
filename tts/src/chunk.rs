pub use forced_alignment::transcript::Segment;

/// One synthesized sentence: MP3-encoded audio + transcript segment with timing.
pub struct AudioSegment {
    /// Zero-based position of this sentence in the stream.
    pub index: usize,
    /// MP3-encoded audio bytes.
    pub audio: Vec<u8>,
    /// Duration in seconds.
    pub duration: f64,
    /// Sentence text and timing. `words` is empty for engines that don't produce
    /// word-level timestamps; populated after forced alignment for engines that do.
    pub transcript: Segment,
    /// True if this is the last sentence in its paragraph.
    pub paragraph_end: bool,
}
