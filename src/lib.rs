mod engine;
mod error;
pub(crate) mod splitter;

pub use engine::{G2pEngine, PhonemeChunk};
pub use error::G2pError;
