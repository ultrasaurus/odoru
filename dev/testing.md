# Testing

## Unit tests

Run without venv — no Python required:

```bash
cargo test --lib           # splitter, audio_cache, normalizer, etc.
cargo test -p tts --lib    # tts crate only
```

## Integration tests

All marked `#[ignore]` — require the venv active:

```bash
source .venv/bin/activate
cargo test --test integration -- --ignored
```

Even `MockBackend` requires venv because `TtsEngine::build()` always initializes Python via PyO3 before constructing any backend.
