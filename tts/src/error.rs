use thiserror::Error;

#[derive(Debug, Error)]
pub enum TtsError {
    /// Python interpreter failed to start, venv not found, or backend couldn't be imported.
    #[error("TTS engine initialisation failed: {0}")]
    PythonInit(String),

    /// The backend raised an exception processing a specific sentence.
    /// The stream continues after this — only this sentence is skipped.
    #[error("synthesis failed on sentence {index}: {cause}\n  sentence: {sentence:?}")]
    SynthesisFailed {
        index: usize,
        sentence: String,
        cause: String,
    },

    /// The requested voice name was not found in the backend's voice list.
    #[error("unknown voice: {0:?}")]
    UnknownVoice(String),

    /// The internal channel to the Python thread was dropped (engine was shut down).
    #[error("TTS engine stream closed unexpectedly")]
    StreamClosed,
}
