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

## Planned improvements

### Authoring

#### Done
- Documents are editable — textarea editor with Edit/Preview toggle; auto-save on debounce; re-synth on Preview; see [editing.md](editing.md)
- URL-fetched docs are editable (correct imperfect scraping)
- Title and source URL editable for all docs
- `PATCH /documents/:id` supports `content`, `plain_text`, `title`, `source_url`, `authors`, `date`

#### Deferred
- Outline view for editor
- Transclusion authoring (paste-as-transclusion, refs.json resolution) — see [transclusion.md](transclusion.md)

### Small authoring bugs / improvements
- Open button in Documents panel: navigate to reader (or editor?) for that document
- upload text/markdown docs to synthesize

#### Open questions for authoring

- voice picker in reader: wait for more experience with real authoring


**Sentence splitting (`tts/src/splitter.rs` + `app/frontend/src/markdown.ts`)**

Paragraphs split on `\n\n`; within each paragraph, single newlines are hard breaks and
`unicode_sentences()` / `Intl.Segmenter` find sentence boundaries. Both sides apply the
same two post-processing rules to keep indices in sync:

- **Outline label merge** — a short all-caps or all-lowercase-Roman-numeral label
  (`I.`, `XIV.`, `ii.`, `A.`) is merged with the sentence that follows it.
  Fixes the UAX #29 behaviour where `"I. Introduction"` splits into `["I.", "Introduction"]`.
- **No-alpha filter** — sentences with no alphabetic characters are dropped (see below).

The client maps incoming audio segments to `pendingSpans` by **arrival order** (`receivedCount++`
in `player.ts`), not by `msg.index`. Any sentence skipped server-side must be skipped client-side
too, or highlighting drifts. When adding a new server-side filter, add the matching filter in
`splitLines` in `markdown.ts`.

**Sentence filtering**

The engine and the client both skip sentences with no alphabetic content. This handles
footnote markers (`*1*`, `[12]`) that trafilatura includes in `plain_text` as standalone
sentences after Unicode sentence splitting. Skipping symmetrically on both sides keeps
segment indices in sync.

**Pronunciation overrides**

`tts_overrides.txt` at the workspace root defines per-token pronunciation fixes for the F5
normalizer (two-column: `match  replacement`, case-insensitive, `#` comments).

The override table is held in a process-global `Arc<RwLock<HashMap>>` (in
`tts/src/f5/normalizer.rs`) initialized once from disk and live-reloadable — no server restart
needed. `normalize()` acquires a read lock; `add_override` / `remove_override` acquire a write
lock and rewrite `tts_overrides.txt` immediately.

On override change (`POST /overrides` or `DELETE /overrides/:word`):
1. Normalizer map updated in-memory and written to disk.
2. All `~/.odoru/audio/*.json` sidecar files whose stored `text` contains the word are marked
   `invalid: true, invalid_reason: "override"` (the entry is skipped on next cache lookup).
3. The in-memory `SegmentCache` (`DashMap` in `AppState`) is cleared entirely.

The reader's "Fix pronunciation" popover (author path only): select a word in the transcript,
type the phonetic replacement, Save. The server updates the override and the client reloads the
document, triggering re-synthesis of affected sentences (from the F5 model) while unaffected
sentences resolve from disk cache.

**Future:** a mark-and-sweep GC pass should scan `~/.odoru/audio/` for `invalid: true` entries
(and optionally entries older than a TTL) and delete the `.mp3` + `.json` pair. The `invalid_reason`
field leaves room for additional invalidation sources (`"manual"`, `"ttl"`).

**Mutable text and audio cache invalidation**

If the user edits a sentence, the cached audio for that sentence is stale.
The audio cache key is SHA-256(normalized_text + voice_cache_key) — it will
naturally miss on changed text. `voices.json` status moves to `stale` for all
voices when `PATCH /documents/:id` touches the `content` field; old audio remains
playable with a warning badge. Per-sentence dirty state is more precise but complex.
The future versioning vision (retaining original document) may change what
"invalidation" means entirely. Not needed for now — defer.


### Open questions / future work
- WS streaming doesn't persist to the audio disk cache (segments are in-memory only). Originally fine when WS was for short snippets, but now authors can seek into long documents via Preview and synthesize large spans that vanish on server restart if the bg job hasn't reached them yet. Consider having WS-synthesized segments also write to the disk cache.

### Polish / small bugs
- Error bar: currently only in Edit view; should be in a shared layout wrapper
- pause/play icons — easy to see state + what action will happen
- Synthesis time display: ~2161m 45s should be H:MM:SS
- Audio disk cache: no eviction — grows unbounded; needs a cleanup strategy (mark-and-sweep; entries already support `invalid: bool` / `invalid_reason` fields for this)

#### TTS improvements
- Abbreviation edge cases: `D. C.`, `pp.` not yet handled in sentence splitter

## Known issues
- Segfault on CLI exit when `--audio` is used (PyO3/tokio shutdown ordering)
  — all output written successfully before crash
- F5 voice switching reloads the full model (~30s penalty)
- lowercase roman numbers aren't spoken as such -- would need per document
  overrides for when they are sample data (as in authorship paper) or
  kisses (xx or xxx).