//! Text normalization for F5-TTS.
//!
//! F5-TTS requires some text preprocessing that Kokoro handles natively.
//! Shared proper noun overrides will be refactored here later.

/// Normalize `text` for F5-TTS synthesis.
pub fn normalize(text: &str) -> String {
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_passthrough() {
        assert_eq!(normalize("Hello world."), "Hello world.");
    }
}
