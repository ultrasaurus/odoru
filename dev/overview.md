# odoru — Architecture & Development Notes

## What it is (& will be)
A hypertext audio reading (and authoring) app. Fetches web articles, synthesizes them to speech,
plays back with synchronized transcript highlighting. Name means "dance/leap/journey" in Japanese.

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
See [tts-backend/overview.md](tts-backend/overview.md). Python environment setup in [tts-backend/python-setup.md](tts-backend/python-setup.md).

## Frontend
See [frontend.md](frontend.md).

## Document store (`util/src/documents.rs`)
- Location: `~/.odoru/documents/<uuid>/`
- Files per document:
  - `document.md` — YAML frontmatter (`id`, `status`, `source_url`, `title`, `authors`, `date`, `description`, `cached_at`, `publish`, `content_hash`) + markdown body
  - `document.txt` — plain text for TTS
  - `source.html` — originally fetched HTML (used for content hash; display deferred)
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

## Background jobs (`app/src/jobs.rs`)
- Location: `~/.odoru/jobs/<id>.json`
- Synthesize a document in the background, populating the audio disk cache sentence-by-sentence
- Per-sentence lock in TtsEngine prevents duplicate synthesis with live WS sessions
- Status: `pending | in_progress | done | error | cancelled`
- `POST /jobs` deduplicates: same text+voice returns existing job unless error/cancelled
- Jobs that were `in_progress` at server shutdown reset to `pending` on reload (preserving
  `completed_sentences`); on startup, pending jobs with an `article_id` auto-restart
  sequentially
- On completion: updates `voices.json` via `update_voice_status`
- Cancel flag (`Arc<AtomicBool>`) is in-memory only; task stops at next sentence boundary
- `text_preview`, `article_id`, `article_title` use `#[serde(default)]` so old entries load

## Planned improvements

### Authoring


#### Text content without fetching URL ###
- Pasted text is ephemeral and can be edited until Synthesize is pressed, then:
  - text area becomes non-editable 
  - the Document is created, job is started
- future: upload

### Hints on how to pronounce words ###
- Mispronounced words: no UI for `tts_overrides.txt` edits
- will need to invalidate cache entries

#### Documents are editable ###
- `PATCH /documents/:id` with stale voice transition (for content edits)

#### Results from URL fetch are editable ###
so text can be adjusted if scraping is imperfect
  1. Markdown editor for content with preview option
  2. Outline view for editor 

### Small authoring bugs / improvements
- pause/cancel/resume/delete jobs
- Open button in Documents panel: navigate to reader (or editor?) for that document
- Publish voice picker in queue row: show all voices (including in-progress), not just those with `duration`

#### Open questions for authoring

- voice picker in reader: wait for more experience with real authoring


**Mutable text and audio cache invalidation**

If the user edits a sentence, the cached audio for that sentence is stale.
The audio cache key is SHA-256(normalized_text + voice_cache_key) — it will
naturally miss on changed text. `voices.json` status moves to `stale` for all
voices when `PATCH /documents/:id` touches the `content` field; old audio remains
playable with a warning badge. Per-sentence dirty state is more precise but complex.
The future versioning vision (retaining original document) may change what
"invalidation" means entirely. Not needed for now — defer.


### Polish / small bugs
- Error bar: currently only in Edit view; should be in a shared layout wrapper
- pause/play icons — easy to see state + what action will happen
- Synthesis time display: ~2161m 45s should be H:MM:SS
- Audio disk cache: no eviction — grows unbounded; needs a cleanup strategy

#### TTS improvements
- Abbreviation edge cases: `D. C.`, `pp.` not yet handled in sentence splitter
- Roman numeral / outline-style headers (`I.`, `II.`, `A.`, `B.`) cause sentence splitting
  mismatch between server (`unicode_segmentation`) and client (`Intl.Segmenter`) — server splits
  `"I. INTRODUCTION"` as two sentences, client may produce one; causes spans to activate out of
  sync with audio. Affects "Augmenting Human Intellect" and similar structured documents.

## Known issues
- Segfault on CLI exit when `--audio` is used (PyO3/tokio shutdown ordering)
  — all output written successfully before crash
- F5 voice switching reloads the full model (~30s penalty)
