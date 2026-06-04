# odoru — Architecture & Development Notes

## What it is (& will be)
A hypertext audio reading (and authoring) app. Fetches web articles, synthesizes them to speech,
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
See [tts-backend/overview.md](tts-backend/overview.md). Python environment setup in [tts-backend/python-setup.md](tts-backend/python-setup.md).

## Frontend
See [frontend.md](frontend.md).

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

## Next planned improvements

### Not yet implemented (discussed)
- Background Queue: show article title + URL per job instead of text_preview
- Jobs: store article URL in job record; auto-restart pending jobs on server startup
  by looking up text from article store (currently requires manual re-submit)
- Article store: expose `synthesized_voices` list in `GET /doc` response for UI use
- Audio disk cache: no eviction — grows unbounded; needs a cleanup strategy
- Error bar: currently only in New view; should be in a shared layout wrapper
- Mispronounced words: no UI for `tts_overrides.txt` edits
- Abbreviation edge cases: `D. C.`, `pp.` not yet handled in sentence splitter

## Known issues
- Segfault on CLI exit when `--audio` is used (PyO3/tokio shutdown ordering)
  — all output written successfully before crash
- F5 voice switching reloads the full model (~30s penalty)
