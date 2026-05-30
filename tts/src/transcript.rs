use serde::{Deserialize, Serialize};

/// Word-level timestamp data, compatible with the WhisperX `AlignedTranscriptionResult` JSON format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    /// Segments, each covering a continuous speech utterance.
    pub segments: Vec<Segment>,

    /// Flat list of every word across all segments — a convenience duplicate
    /// of the nested `words` fields, useful for simple iteration.
    pub word_segments: Vec<Word>,

    /// BCP-47 language code detected or specified at transcription time (e.g. `"en"`).
    pub language: String,
}

/// A single aligned segment (`SingleSegment`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    /// Segment start time in seconds.
    pub start: f64,

    /// Segment end time in seconds.
    pub end: f64,

    /// Full transcript text for this segment.
    pub text: String,

    /// Word-level alignments within this segment.
    pub words: Vec<Word>,

    /// Speaker label — only present when diarization was enabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
}

/// A single word with timing and alignment confidence (`SingleWord`).
///
/// `start` and `end` may be absent for words that could not be aligned
/// (e.g. numbers at sentence boundaries that whisperX sometimes can't place).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Word {
    /// The word string. Punctuation is attached to the preceding word (e.g. `"annotations,"`).
    pub word: String,

    /// Word start time in seconds. May be missing if alignment failed for this token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<f64>,

    /// Word end time in seconds. May be missing if alignment failed for this token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<f64>,

    /// Mean CTC alignment probability for this word's characters (0.0 – 1.0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,

    /// Speaker label — only present when diarization was enabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "segments": [{
            "start": 0.11, "end": 3.498,
            "text": " It may contain annotations, additions and footnotes",
            "words": [
                {"word": "It",           "start": 0.11,  "end": 0.17,  "score": 0.958},
                {"word": "may",          "start": 0.211, "end": 0.352, "score": 0.884},
                {"word": "contain",      "start": 0.392, "end": 0.755, "score": 0.903},
                {"word": "annotations,", "start": 0.876, "end": 1.622, "score": 0.803},
                {"word": "additions",    "start": 2.368, "end": 2.832, "score": 0.812},
                {"word": "and",          "start": 2.893, "end": 2.973, "score": 0.834},
                {"word": "footnotes",    "start": 3.034, "end": 3.498, "score": 0.904}
            ]
        }],
        "word_segments": [
            {"word": "It",           "start": 0.11,  "end": 0.17,  "score": 0.958},
            {"word": "may",          "start": 0.211, "end": 0.352, "score": 0.884},
            {"word": "contain",      "start": 0.392, "end": 0.755, "score": 0.903},
            {"word": "annotations,", "start": 0.876, "end": 1.622, "score": 0.803},
            {"word": "additions",    "start": 2.368, "end": 2.832, "score": 0.812},
            {"word": "and",          "start": 2.893, "end": 2.973, "score": 0.834},
            {"word": "footnotes",    "start": 3.034, "end": 3.498, "score": 0.904}
        ],
        "language": "en"
    }"#;

    #[test]
    fn roundtrip() {
        let parsed: Transcript = serde_json::from_str(SAMPLE).unwrap();

        assert_eq!(parsed.language, "en");
        assert_eq!(parsed.segments.len(), 1);
        assert_eq!(parsed.segments[0].words.len(), 7);
        assert_eq!(parsed.word_segments.len(), 7);

        let first_word = &parsed.segments[0].words[0];
        assert_eq!(first_word.word, "It");
        assert_eq!(first_word.start, Some(0.11));
        assert!((first_word.score.unwrap() - 0.958).abs() < f64::EPSILON);

        // Re-serialise and round-trip again to confirm symmetry.
        let json = serde_json::to_string(&parsed).unwrap();
        let reparsed: Transcript = serde_json::from_str(&json).unwrap();
        assert_eq!(reparsed.segments[0].text, parsed.segments[0].text);
    }
}
