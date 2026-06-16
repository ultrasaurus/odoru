# odoru — Architecture & Development Notes

## What it is (& will be)
A hypertext audio reading (and authoring) app. Fetches web articles, synthesizes
them to speech, plays back with synchronized transcript highlighting.
Name means "dance/leap/journey" in Japanese.

Features currently supported:
* Create/Edit document from URL or type/paste into new doc - [editing.md](editing.md)
* Listen to a documet, including seeking anywhere, even while synth is in progress
* Publish a collection of documents and export to SPA - [export.md](export.md)

## Workspace layout
```
odoru/
  tts/        — multi-backend streaming TTS library (the main crate)
  app/        — Axum WebSocket server + REST API, serves frontend
  cli/        — `dl` binary: fetch URL or local file, synthesize to MP3
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
See [tts-backend/overview.md](tts-backend/overview.md).

Python environment setup in [tts-backend/python-setup.md](tts-backend/python-setup.md).

## Frontend
See [frontend.md](frontend.md).

## Document store (`util/src/documents.rs`)
- Location: `~/.odoru/documents/<uuid>/`
- Files per document:
  - `document.md` — YAML frontmatter (`id`, `status`, `source_url`, `title`, `authors`, `date`, `description`, `cached_at`, `publish`, `content_hash`) + markdown body
  - `document.txt` — plain text for TTS
  - `source.html` — originally fetched HTML (used for content hash; display deferred) — absent for text docs
  - `voices.json` — per-voice synthesis state (see below)
- Identity is a UUID assigned at creation — decoupled from URL and content
- `source_url` is provenance metadata, not an identity field
- `status`: `fetching | ready | error` — set at creation, updated on fetch completion

## Voice state (`voices.json`)
Per-document, keyed by voice ID (e.g. `"f5:sarah"`):
```json
{
  "f5:sarah": { "status": "ready", "duration": 312.4, "job_id": "...", "published": true },
  "f5:nova":  { "status": "in_progress", "job_id": "..." }
}
```
- Statuses: `in_progress | ready | stale | error`
- `stale`: content changed since synthesis — old audio still playable, shown with warning badge
- `published: true` on at most one voice; combined with `publish` flag in frontmatter
- Written by: WS handler on session done, job runner on job done
- Concurrent writes protected by per-document `RwLock` in `AppState`

## Document indexes (`util/src/index.rs`)
- Location: `~/.odoru/index/source_url.json` and `content_hash.json`
- Loaded into memory at startup (`DocumentIndex` in `AppState`)
- Reads: no lock (read directly from in-memory maps)
- Writes: `RwLock` write guard, then async flush via write-to-temp-then-rename
- On flush failure: logs `error!`, writes `.rebuild-needed` sentinel
- On startup with sentinel: rebuilds by scanning all document directories

## API
See [protocol.md](protocol.md).

### App state
- `ODORU_BACKEND` env var: "kokoro" (default), "f5", or "both"
- `ODORU_WORKERS` env var: F5 worker count (default: 1)
- `VOICES_DIR` env var: path to voices directory
- `KOKORO_MODEL_DIR` env var: path to Kokoro model (default: `~/.kokoro`)
- Both engines held in AppState simultaneously when `ODORU_BACKEND=both`
- In-memory segment cache: SHA-256(voice_id + "|" + text) → Vec<CachedSegment>
- `doc_index`: in-memory `DocumentIndex`
- `voice_locks`: per-document `RwLock` for `voices.json` writes, keyed by UUID
- Pronunciation overrides: live-reloadable `RwLock<HashMap>` (see below)

## Background jobs (`app/src/jobs.rs`)
- Location: `~/.odoru/jobs/<id>.json`
- Synthesize a document in the background, populating the audio disk cache sentence-by-sentence
- Per-sentence lock in TtsEngine prevents duplicate synthesis with live WS sessions
- Status: `pending | in_progress | done | error | paused`
- `POST /jobs` deduplicates: same text+voice returns existing job unless `error`; a `paused`
  job is returned as-is (not auto-resumed) — only `POST /jobs/:id/resume` restarts it
- Jobs that were `in_progress` at server shutdown reset to `pending` on reload (preserving
  `completed_sentences`); on startup, pending jobs with an `article_id` auto-restart
  sequentially
- On completion: updates `voices.json` via `update_voice_status`
- Pause: `POST /jobs/:id/pause` sets a stop flag (`Arc<AtomicBool>`); the task stops at the
  next sentence boundary and marks the job `paused`, preserving `completed_sentences` so
  `POST /jobs/:id/resume` re-runs quickly via the disk cache. Paused jobs are never
  auto-resumed.
- Delete: `JobStore::delete()` removes a job's in-memory and on-disk state; if a task is
  running, signals it to stop without re-persisting. Used by `DELETE /documents/:id` (all
  jobs for the doc) and `DELETE /documents/:id/voices/:voice_id` (that voice's job).
- `text_preview`, `article_id`, `article_title` use `#[serde(default)]` so old entries load
