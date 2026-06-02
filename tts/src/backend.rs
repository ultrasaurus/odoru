use std::path::PathBuf;

/// A speaker voice — backend-specific configuration.
#[derive(Debug, Clone)]
pub enum Voice {
    /// Kokoro voice — named preset (e.g. "am_puck", "af_heart").
    Kokoro {
        /// Voice name matching a .bin file in the model's voices/ directory.
        name: String,
    },

    /// F5-TTS MLX voice — reference audio clip plus synthesis parameters.
    F5Tts {
        /// Identifier, lowercase (e.g. "scott"). Used for cache keys and lookup.
        name: String,
        /// Path to a reference audio clip (mono 24kHz wav, 5-10 seconds).
        voice_ref: PathBuf,
        /// Exact transcript of the reference clip — used for voice alignment.
        ref_text: String,
        /// Speech speed multiplier. 1.0 = normal, 0.85 = slightly slower.
        speed: f32,
        /// Classifier-free guidance strength. Typical range: 1.0 – 3.0.
        cfg_strength: f32,
    },

    /// Mock voice — pairs with Backend::Mock. No config needed.
    Mock,
}

impl Voice {
    /// Construct a Kokoro voice.
    pub fn kokoro(name: impl Into<String>) -> Self {
        Voice::Kokoro { name: name.into() }
    }

    /// Construct an F5Tts voice with default speed and cfg_strength.
    pub fn f5(
        name: impl Into<String>,
        voice_ref: impl Into<PathBuf>,
        ref_text: impl Into<String>,
    ) -> Self {
        Voice::F5Tts {
            name: name.into().to_lowercase(),
            voice_ref: voice_ref.into(),
            ref_text: ref_text.into(),
            speed: 0.85,
            cfg_strength: 2.0,
        }
    }

    /// The voice name in lowercase.
    pub fn name(&self) -> &str {
        match self {
            Voice::Kokoro { name } => name,
            Voice::F5Tts { name, .. } => name,
            Voice::Mock => "mock",
        }
    }

    /// A stable string key for cache keying — includes all params that affect output.
    pub fn cache_key(&self) -> String {
        match self {
            Voice::Kokoro { name } => format!("kokoro:{name}"),
            Voice::F5Tts { name, speed, cfg_strength, .. } =>
                format!("f5:{name}:{speed}:{cfg_strength}"),
            Voice::Mock => "mock".into(),
        }
    }
}

/// Engine-level backend configuration.
#[derive(Debug, Clone)]
pub enum Backend {
    /// Kokoro TTS — pure Rust ONNX inference + Python G2P (misaki).
    Kokoro {
        /// Directory containing model.onnx, tokenizer.json, voices/.
        model_dir: PathBuf,
        /// Voice name (e.g. "am_puck").
        voice: String,
        /// Speed multiplier (1.0 = normal).
        speed: f32,
    },

    /// F5-TTS via MLX — 4-bit quantized, ~363 MB per worker.
    F5Tts {
        /// Available voices. At least one required.
        voices: Vec<Voice>,
        /// Number of parallel synthesis workers. Default 1.
        workers: usize,
    },

    /// Sine-wave mock — no model weights needed, useful for testing.
    Mock,
}

impl Backend {
    /// Whether this backend requires Python to be initialized.
    /// All backends currently need Python.
    pub fn needs_python(&self) -> bool {
        true
    }

    pub fn workers(&self) -> usize {
        match self {
            Backend::F5Tts { workers, .. } => *workers,
            _ => 1,
        }
    }
}

impl Default for Backend {
    fn default() -> Self {
        Backend::Mock
    }
}
