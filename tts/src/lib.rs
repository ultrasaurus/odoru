//! # tts
//!
//! Multi-backend streaming TTS library.
//!
//! Supports Kokoro (Rust ONNX + Python G2P) and F5-TTS (Python MLX),
//! with a Mock backend for testing.
//!
//! ## Quick start
//!
//! ```no_run
//! use tts::{TtsEngine, Backend};
//! use futures::StreamExt;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), tts::TtsError> {
//!     let engine = TtsEngine::builder()
//!         .backend(Backend::Mock)
//!         .build()?;
//!
//!     let mut stream = engine.synthesize("Hello world.", "mock");
//!     while let Some(result) = stream.next().await {
//!         let seg = result?;
//!         println!("[{:.3}–{:.3}] {}", seg.transcript.start, seg.transcript.end, seg.transcript.text);
//!     }
//!     Ok(())
//! }
//! ```

// Shared infrastructure
pub mod splitter;
pub mod chunk;
pub mod backend;
pub mod transcript;

// Python integration (internal)
pub(crate) mod python;

// TTS engine — public API
mod error;
mod engine;
mod mock;

pub use error::TtsError;
pub use engine::{TtsEngine, TtsEngineBuilder, AudioStream, TtsBackend};

// Backends
pub mod kokoro;
pub mod f5;

// G2P engine (used by Kokoro, exposed for examples/tests)
mod g2p;
pub use g2p::{G2pEngine, PhonemeChunk, G2pError};

// Convenience re-exports
pub use chunk::{AudioSegment, Segment};
pub use backend::{Backend, Voice};
pub use kokoro::save_wav_all;
