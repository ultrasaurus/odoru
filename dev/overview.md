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
  util/       — shared Rust utilities: frontmatter, voice loading, article store, audio cache
  voices/     — F5 voice definitions (sarah/, f5-am-puck/, etc.)
  tts_overrides.txt — pronunciation overrides for F5 normalizer
```

## Running
```bash
source .venv/bin/activate
cargo run -p app                          # Kokoro only (default)
ODORU_BACKEND=f5 cargo run -p app         # F5 only
ODORU_BACKEND=both cargo run -p app       # Both backends simultaneously
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
- Only used for F5 (Kokoro is fast enough to skip)
- `exists(key)` checks file presence only (no reads) — used by `all_audio_cached`
- `lookup(key)` reads full samples — used during synthesis
- Enables resumable synthesis: Ctrl+C and restart, completed sentences load instantly

## Article store (`util/src/cache.rs`)
- Location: `~/.odoru/articles/<request-url-slug>/`
- Files: `article.md` (YAML frontmatter + markdown body) + `article.txt` (plain text)
- Frontmatter fields: url, title, authors, date, description, cached_at, synthesized_voices
- **Key is always the request URL** — trafilatura's reported URL is unreliable, never used as key
- `synthesized_voices`: list of voice IDs (e.g. `["f5:sarah"]`) for which all sentences are
  synthesized. Populated lazily by `GET /doc` after `all_audio_cached` returns true.
  Makes subsequent `GET /doc` calls instant (no Python, no stat calls).

## REST API (`app/src/main.rs`)
```
GET  /voices          → { voices: [{id, name, backend, description}] }
GET  /doc?url=&voice= → { url, title, authors, date, plain_text, content,
                           cached: { content: bool, audio: voice_cache_key|null } }
GET  /ws              → WebSocket upgrade
POST /jobs            → { text, voice } → job (deduplicates by text+voice)
GET  /jobs            → [job, ...]
GET  /jobs/:id        → job
DELETE /jobs/:id      → cancel job
```

### WebSocket protocol
Client → server (voice must be prefixed):
```json
{ "text": "...", "voice": "f5:sarah" }
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
- `ODORU_BACKEND` env var: "kokoro" (default), "f5", or "both"
- `ODORU_WORKERS` env var: F5 worker count (default: 1)
- `VOICES_DIR` env var: path to voices directory
- `KOKORO_MODEL_DIR` env var: path to Kokoro model (default: `~/.kokoro`)
- Both engines held in AppState simultaneously when `ODORU_BACKEND=both`
- In-memory segment cache: SHA-256(voice_id + "|" + text) → Vec<CachedSegment>

## Background jobs (`app/src/jobs.rs`)
- Location: `~/.odoru/jobs/<id>.json`
- Synthesize an article in the background, populating the audio disk cache sentence-by-sentence
- Per-sentence lock in TtsEngine prevents duplicate synthesis with live WS sessions
- Status: `pending | in_progress | done | error | cancelled`
- `POST /jobs` deduplicates: same text+voice returns existing job unless error/cancelled
- Jobs that were `in_progress` at server shutdown reset to `pending` on reload (preserving
  `completed_sentences`); they restart when the client re-submits via `POST /jobs`
- Cancel flag (`Arc<AtomicBool>`) is in-memory only; task stops at next sentence boundary
- `text_preview`: first 80 chars, for display. `#[serde(default)]` so old entries load.

## Frontend (`app/frontend/src/`)
- `main.ts` — two views: reader (hardcoded Engelbart article) and New (arbitrary URL/text)
- `player.ts` — WebSocket client, AudioContext queue, transcript highlighting
- `style.css` — dark theme, CSS variables
- Built with Vite + TypeScript, output to `app/frontend/dist/`

### Reader view
- Pre-renders all sentences as gray `segment pending` spans immediately after doc fetch
- Player activates each span in place as audio arrives (removes `pending` class, wires click)
- "Synthesize in background" button shown when audio not fully cached (all backends)
- Job progress shown in header; polls `GET /jobs/:id` every 4s while running
- `viewCleanup` stops poll timers when navigating to New view

### New view
- URL fetch + text area + voice picker + "Synthesize in background" checkbox
- Checkbox unchecked: live streaming WS synthesis
- Checkbox checked: `POST /jobs`, progress shown in transcript area, polls every 4s
- Background Queue section below card: lists all jobs, cancel button on active jobs,
  polls `GET /jobs` every 10s
- `viewCleanup` stops all timers when navigating to Reader view
- Download enabled on `onSynthDone` (synthesis stream complete), not on playback end
- `downloadFilename()` evaluated at click time (lazy), not at view init

### Player timing model
- `AudioContext` plays segments as they arrive (streaming)
- `startTracking()` polls `AudioContext.currentTime` to update progress + highlighting
- `onSynthDone` fires when WS sends `{done: true}` — enables download
- `onEnded` fires when `done === true` AND playback position >= last segment end
- Seek: click transcript sentence → jump to that segment's start time
- `ws.onclose` handler: non-clean close fires `onError` so UI surfaces server crash

## Next planned improvements

### Markdown rendering
The `content` field is trafilatura-extracted markdown. It has headings (`#`, `##`),
paragraphs, and some bold/italic. A lightweight client-side renderer (marked.js or
similar) would work. The `plain_text` is what gets synthesized and the reader
currently shows plain text sentences aligned to the rendered text.

### Not yet implemented (discussed)
- Background Queue: show article title + URL per job instead of text_preview
- Jobs: store article URL in job record; auto-restart pending jobs on server startup
  by looking up text from article store (currently requires manual re-submit)
- Article store: expose `synthesized_voices` list in `GET /doc` response for UI use
- Audio disk cache: no eviction — grows unbounded; needs a cleanup strategy
- Error bar: currently only in New view; should be in a shared layout wrapper
- Mispronounced words: no UI for `tts_overrides.txt` edits

## Known issues
- Segfault on CLI exit when `--audio` is used (PyO3/tokio shutdown ordering)
  — all output written successfully before crash
- F5 voice switching reloads the full model (~30s penalty)

## Python environment setup
```bash
python3.12 -m venv .venv
source .venv/bin/activate
pip install "misaki[en]" click trafilatura f5-tts-mlx soundfile
python -m spacy download en_core_web_sm
```
