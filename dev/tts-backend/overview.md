# TTS Backends

## Kokoro (default)
- Pure Rust ONNX inference via `ort` crate
- G2P via misaki (Python/PyO3) on a dedicated worker thread
- ~0.2 sec/word generation speed on M1
- Voices: `~/.kokoro/voices/*.bin` — 28 English voices (af_, am_, bf_, bm_ prefixes)
- Config: `KOKORO_MODEL_DIR` env var (default: `~/.kokoro`)

## F5-TTS
- MLX inference via Python (`f5_tts_mlx`)
- **Threading**: each worker is a dedicated `std::thread` (NOT spawn_blocking) because
  MLX creates a GPU stream per OS thread and it cannot be shared
- ~2.5 sec/word generation speed on M1 (varies hugely by sentence complexity)
- Voice switching reloads the entire model (MLX retains internal state otherwise)
- Voices loaded from `voices/` directory (or `$VOICES_DIR`)
- Each voice dir: `voice.md` (YAML frontmatter: transcript, speed, cfg_strength) + `ref.wav`
- `tts_overrides.txt`: word→pronunciation map, applied before synthesis

## Mock
- Sine wave, instant, no model weights needed, for testing

## Voice IDs
Voices are identified by prefixed strings: `"f5:sarah"`, `"kokoro:am_puck"`.
The prefix is required in all API calls — unprefixed names are rejected.
`GET /voices` returns a flat list ordered F5 first, then Kokoro.

## TtsEngine API
```rust
let engine = TtsEngine::builder()
    .backend(Backend::F5Tts { voices: vec![voice], workers: 1 })
    .build()?;

let mut stream = engine.synthesize("Hello world.", "sarah");  // bare name, no prefix
while let Some(result) = stream.next().await {
    let seg = result?;
    // seg.index, seg.samples, seg.sample_rate, seg.transcript.{start,end,text}, seg.paragraph_end
}

engine.voice_names()         // Vec<String> — bare names
engine.voice_cache_key(name) // Option<String> — e.g. "f5:sarah:0.85:1.5"
engine.all_audio_cached(text, name) // Option<bool> — checks disk cache via exists() (no reads)
```

### Per-sentence synthesis lock
`TtsEngine` holds a `DashMap<SentenceCacheKey, Arc<Mutex<()>>>`. Before synthesizing any
sentence, the lock is acquired so two concurrent callers (WS session + background job) cannot
synthesize the same sentence simultaneously — the second waits and gets a disk cache hit.

### TtsBackend trait
```rust
pub trait TtsBackend: Send + Sync {
    fn synthesize_sentence(&self, text: &str, voice: &Voice, index: usize)
        -> Result<(Vec<f32>, u32, f64), TtsError>;
}
```

## Audio disk cache (`tts/src/audio_cache.rs`)
- Location: `~/.odoru/audio/`
- Files: `<hash>.f32` (raw f32le samples) + `<hash>.json` (metadata)
- Key: SHA-256(normalized_text + "|" + voice_cache_key)
- Only used for F5 — Kokoro does not yet opt in (seeking in long Kokoro articles re-synthesizes)
- `exists(key)` checks file presence only (no reads) — used by `all_audio_cached`
- `lookup(key)` reads full samples — used during synthesis
- Enables resumable synthesis: Ctrl+C and restart, completed sentences load instantly
