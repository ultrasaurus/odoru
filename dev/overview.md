# odoru — Architecture & Development Notes

## What it is
A hypertext audio reading app. Fetches web articles, synthesizes them to speech,
plays back with synchronized transcript highlighting. Name means "dance/leap/journey" in Japanese.

## Workspace layout
```
odoru/
  tts/        — multi-backend streaming TTS library (the main crate)
  app/        — Axum WebSocket server + REST API, serves frontend
  cli/        — `dl` binary: fetch URL or local file, synthesize to WAV
  dl/         — fetch + extract articles via trafilatura (Python)
  py-venv/    — shared PyO3 utilities
  config/     — shared AudioConfig (sample_rate, silence durations)
  util/       — shared Rust utilities: frontmatter, voice loading, article cache, audio cache
  voices/     — F5 voice definitions (sarah/, f5-am-puck/, etc.)
  tts_overrides.txt — pronunciation overrides for F5 normalizer
```

## Running
```bash
source .venv/bin/activate
cargo run -p app                          # Kokoro backend (default)
ODORU_BACKEND=f5 cargo run -p app         # F5 backend
cargo run --bin dl -- --audio --backend f5 --voice sarah data/abstract.txt
cargo test
cargo test --test integration -- --ignored  # needs venv active
```

## TTS backends

### Kokoro (default)
- Pure Rust ONNX inference via `ort` crate
- G2P via misaki (Python/PyO3) on a dedicated worker thread
- ~0.2 sec/word generation speed on M1
- Voices: `~/.kokoro/voices/*.bin` — 28 English voices (af_, am_, bf_, bm_ prefixes)
- Config: `KOKORO_MODEL_DIR` env var (default: `~/.kokoro`)

### F5-TTS
- MLX inference via Python (`f5_tts_mlx`)
- **Threading**: each worker is a dedicated `std::thread` (NOT spawn_blocking) because
  MLX creates a GPU stream per OS thread and it cannot be shared
- ~2.5 sec/word generation speed on M1 (varies hugely by sentence complexity)
- Voice switching reloads the entire model (MLX retains internal state otherwise)
- Voices loaded from `voices/` directory (or `$VOICES_DIR`)
- Each voice dir: `voice.md` (YAML frontmatter: transcript, speed, cfg_strength) + `ref.wav`
- `tts_overrides.txt`: word→pronunciation map, applied before synthesis

### Mock
- Sine wave, instant, no model weights needed, for testing

## TtsEngine API
```rust
let engine = TtsEngine::builder()
    .backend(Backend::F5Tts { voices: vec![voice], workers: 1 })
    .build()?;

let mut stream = engine.synthesize("Hello world.", "sarah");
while let Some(result) = stream.next().await {
    let seg = result?;
    // seg.index, seg.samples, seg.sample_rate, seg.transcript.{start,end,text}, seg.paragraph_end
}

engine.voice_names()  // Vec<String> of available voices
```

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
- Only used for F5 (Kokoro is fast enough to skip)
- Enables resumable synthesis: Ctrl+C and restart, completed sentences load instantly
- CLI shows `[audio cache] hit sentence N, skipping synthesis` on cache hits

## Article cache (`util/src/cache.rs`)
- Location: `~/.odoru/articles/<hostname-slugified-path>/`
- Files: `article.md` (YAML frontmatter + markdown body) + `article.txt` (plain text)
- Frontmatter fields: url, title, authors, date, description, cached_at
- Key derived from URL: `https://ultrasaurus.com/2015/10/foo/` → `ultrasaurus-com-2015-10-foo`
- CLI: `--no-cache` flag bypasses lookup and overwrites

## REST API (`app/src/main.rs`)
```
GET /voices    → { backend: "kokoro"|"f5", voices: [{name, description}] }
GET /doc?url=  → { url, title, authors, date, plain_text, content, cached: bool }
GET /ws        → WebSocket upgrade
```

### WebSocket protocol
Client → server:
```json
{ "text": "...", "voice": "sarah" }
```
Server → client (one per sentence):
```json
{ "index": 0, "transcript": {"start": 0.41, "end": 1.65, "text": "..."}, 
  "audio": "<base64 f32le PCM>", "cached": bool, "paragraph_end": bool }
```
Server → client (when done):
```json
{ "done": true }
```

### App state
- `ODORU_BACKEND` env var: "kokoro" (default) or "f5"
- `ODORU_WORKERS` env var: F5 worker count (default: 1)
- `VOICES_DIR` env var: path to voices directory
- `KOKORO_MODEL_DIR` env var: path to Kokoro model (default: `~/.kokoro`)
- In-memory segment cache: SHA-256(text + voice) → Vec<CachedSegment>

## Frontend (`app/frontend/src/`)
- `main.ts` — UI, URL fetch, voice picker, time estimate, download
- `player.ts` — WebSocket client, AudioContext queue, transcript highlighting
- `style.css` — dark theme, CSS variables
- Built with Vite + TypeScript, output to `app/frontend/dist/`

### Key frontend state
- `activeBackend`: "kokoro" or "f5" (from GET /voices)
- `selectedVoice`: current voice name
- `player.segments`: array of received segments with samples + timestamps
- `player.done`: true once WS sends `{done: true}` — gates `onEnded` callback

### Player timing model
- `AudioContext` plays segments as they arrive (streaming)
- `startTracking()` polls `AudioContext.currentTime` to update progress + highlighting
- `onEnded` only fires when `done === true` AND playback position >= last segment end
- Seek: click transcript sentence → jump to that segment's start time

## Next planned feature: article reader view
The building blocks are all in place:
- `GET /doc` already returns `content` (markdown) alongside `plain_text`
- Each WS segment has `transcript.{start, end, text}` — exact timestamps per sentence
- Player already highlights current sentence during playback

What's needed:
1. Render `content` (markdown) as HTML in a reader pane
2. Map rendered sentences back to WS segments by text matching
3. Click sentence in reader → seek to that timestamp
4. Highlight current sentence in reader view during playback (in addition to transcript pane)
5. "Resume" — remember position in article, restart from there

### Markdown rendering
The `content` field is trafilatura-extracted markdown. It has headings (`#`, `##`),
paragraphs, and some bold/italic. A lightweight client-side renderer (marked.js or
similar) would work. The `plain_text` is what gets synthesized — the reader shows
`content` for formatting, but sentence matching should use `plain_text` sentences
aligned to the rendered text.

## Known issues
- Segfault on CLI exit when `--audio` is used (PyO3/tokio shutdown ordering)
  — all output written successfully before crash
- F5 voice switching reloads the full model (~30s penalty)
- mispronounced words require manual `tts_overrides.txt` edits (no UI yet)

## Python environment setup
```bash
python3.12 -m venv .venv
source .venv/bin/activate
pip install "misaki[en]" click trafilatura f5-tts-mlx soundfile
python -m spacy download en_core_web_sm
```
