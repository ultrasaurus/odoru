//! Python integration infrastructure — internal, not part of the public API.
//!
//! - `setup`: venv detection and sys.path configuration
//! - `bridge`: Python module loading and function calling

pub(crate) mod bridge;
pub(crate) mod setup;
