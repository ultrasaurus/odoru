use thiserror::Error;

#[derive(Debug, Error)]
pub enum G2pError {
    /// Python interpreter failed to start, venv not found, or misaki couldn't be imported.
    #[error("Python initialisation failed: {0}")]
    PythonInit(String),

    /// Misaki raised an exception processing a specific sentence.
    /// The stream continues after this — only this sentence is skipped.
    #[error("G2P failed on sentence {index}: {cause}\n  sentence: {sentence:?}")]
    G2pFailed {
        index: usize,
        sentence: String,
        cause: String,
    },

    /// The internal channel to the Python thread was dropped (engine was shut down).
    #[error("G2P engine stream closed unexpectedly")]
    StreamClosed,
}
