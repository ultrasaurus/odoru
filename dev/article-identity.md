# Article Identity Model

## The Problem

The current article store uses the **request URL as the primary key** — the directory
name is `url_to_slug(url)` and lookup scans for a matching `url` frontmatter field.
This works well for URL-fetched articles because:
- The URL is stable and known before the article exists
- Content rarely changes after fetching
- The URL is a natural globally unique identifier

But the authoring vision requires supporting content that doesn't fit this model:

- **Text snippets**: pasted directly, no source URL, text changes during editing
- **Uploads / local files**: text extracted by external tools; may have a *provenance* URL
  (where the file came from) but that URL didn't produce the text — a different tool did
- **Authored-from-scratch**: no source at all

Also, **Redirects** there may be multiple URLs for one page, causing accidental dups and extra work for the author.

For these cases, basing identity on URL or text hash both fail:
- **URL as key**: no URL to use, or URL doesn't uniquely identify the content version
- **Text hash as key**: text changes during authoring → every edit is a new article,
  old synthesized audio orphaned, jobs pointing at stale records

## Chosen Design: Stable UUID Key

Assign a UUID to each document at creation time. The store directory becomes
`~/.odoru/articles/<uuid>/` rather than `~/.odoru/articles/<url-slug>/`.

- Identity is completely decoupled from content and source
- Text, title, source URL are all mutable metadata
- Jobs reference the document by UUID instead of URL
- `source_url` becomes optional provenance metadata — "where this came from" — not an identity field
- Export slug is title-derived at export time, independent of store key

**Migration**: one-time script saved under `util/bin/migrate_v0_2_uuid_keys.rs`, with the crate
bumped to `0.2.0`. Re-keys existing URL-slug directories to UUID and populates both indexes
from existing frontmatter. Manual run is fine (personal tool, small number of articles).
This establishes the pattern: future breaking store changes get a versioned migration binary
and a crate version bump.

### Deduplication indexes

Two indexes in `~/.odoru/index/`, each a JSON map file:

- `source_url.json`: `url → uuid` — catches re-fetches of the same URL (fast path)
- `content_hash.json`: `sha256(original_fetched_content) → uuid` — catches redirects
  (URL A and URL B resolve to the same content)

On `POST /documents` with a URL: check source_url index first (cheap), then content_hash
(catches redirects). If found, return the existing document immediately — no fetch, no synthesis.

**Content hash stability:** the hash is computed over the originally fetched HTML and never
recomputed. Trafilatura's output can vary between runs for the same page (dynamic page elements,
version differences), but since the hash is taken once at fetch time and stored, it is stable.
The originally fetched HTML is saved as `source.html` in the document directory — used for
hashing and available for debugging. Displaying it to the author is deferred until needed.

**Concurrent writes:** the indexes are loaded into `AppState` at startup and kept in memory.
Reads are served from memory with no lock (`RwLock` read guard). Writes acquire a `RwLock`
write guard, update the in-memory map, then flush to disk asynchronously. On flush failure:

- Log an error loudly (`error!`) so the operator knows
- Write a sentinel file `~/.odoru/index/.rebuild-needed`; if that write also fails, log loudly again
- Continue with the in-memory state correct for the remainder of the session
- On next startup, if `.rebuild-needed` exists: rebuild both indexes by scanning all article
  directories and reading frontmatter, log visibly (`info!`) that a rebuild occurred, then
  delete the sentinel on success

**Crash safety:** index files are written via write-to-temp-then-rename so the old file
survives a crash mid-write (rename is atomic on most filesystems). The sentinel covers the
"flush returned an error" case; atomic rename covers the crash case. See scalability note
in [future-export.md](future-export.md).

The `cached_at` frontmatter field gives the author visibility into when content was last fetched,
so they can judge whether a dedup hit is stale. A `POST /documents/:id/refresh` endpoint
can be added later if force re-fetch becomes a need.

Near-duplicate dedup (article updated slightly → same hash miss) is acceptable for a personal tool.

### Voice state

Per-voice synthesis state is stored in `voices.json` alongside `article.md`, replacing the
`synthesized_voices` and `voice_durations` frontmatter fields.

```json
{
  "f5:sarah": {
    "status": "ready",
    "duration": 312.4,
    "job_id": "uuid"
  },
  "f5:emma": {
    "status": "stale",
    "duration": 298.1,
    "job_id": "uuid"
  },
  "f5:nova": {
    "status": "in-progress",
    "job_id": "uuid"
  }
}
```

Status values:
- `in-progress` — synthesis job running or partially complete
- `ready` — fully synthesized
- `stale` — content changed since synthesis (text edit via PATCH); old audio still playable, shown with a warning badge
- `error` — job failed; author can re-trigger

`duration` is present once ever synthesized and survives the `stale` transition (still reflects
old audio length, good enough for display). `job_id` lets the server return the existing job to
the client if the author re-triggers synthesis on a voice already `in-progress` (noop — return
existing id). It also allows job state recovery on restart.

The voice picker is populated from `voices.json` — author can audition any voice with audio
(including stale), pick a published voice, or delete unwanted voice/document jobs after review.

### API naming

Standardize everything on "document" — current "doc" / "article" naming is inconsistent.

| New endpoint | Replaces |
|---|---|
| `POST /documents` | `GET /doc?url=` (fetch-or-create path) |
| `GET /documents` | `GET /articles` |
| `GET /documents/:id` | `GET /doc?url=` (return path) |
| `PATCH /documents/:id` | (new — edit metadata/content) |
| `DELETE /documents/:id` | (new) |
| `POST /documents/:id/refresh` | (new — force re-fetch) |

Jobs (`POST /jobs`) stay as-is for now; synthesis triggering can be revisited separately.

### Implementation phases

**Phase 1 (UUID keys + voice state):**
- UUID keys, in-memory index, migration script, `voices.json` replacing frontmatter voice fields
- `POST /documents` with a URL returns `{ id }` immediately
- Client polls `GET /documents/:id` until `status: ready`
- Voice picker unblocked; URL fetch → synthesize → listen loop solid end-to-end

**Phase 2 (WS events):**
- Replace polling with a `document_status` WS event: `{ id, status: "fetching" | "ready" | "error", ... }`
- Same channel already used for synthesis — fetch progress is one more event type
- Eliminates polling pattern; architecturally cleaner

**Phase 3 (new input surface area):**
- `PATCH /documents/:id` with stale voice transition
- Snippets, upload, and paste input paths

## Known Constraints

- **Document directory layout** — each `~/.odoru/articles/<uuid>/` directory contains:
  - `article.md` — YAML frontmatter + markdown body
  - `article.txt` — plain text for TTS
  - `source.html` — originally fetched HTML (used for content hash; display to author deferred)
  - `voices.json` — per-voice synthesis state (replaces `synthesized_voices` / `voice_durations` frontmatter fields)

- **Export uses slug as directory name** (`future.md`): `articles/<slug>/meta.json`.
  UUID slugs work but are opaque. A title-derived slug at export time (separate from
  the store key) would be more readable — the export can generate its own slug without
  coupling it to the store key.

- **Jobs store `article_url`** to look up text at auto-restart. With UUID keys,
  this becomes `article_id`. The text lookup changes from `cache::lookup(url)` to
  `cache::lookup_by_id(uuid)` — straightforward.

- **`mark_synthesized` and `update_publish`** address articles by URL today.
  Both need to accept an ID instead (or in addition).

- **`GET /articles`** currently returns `url` as the primary identifier used by the
  frontend to load an article via `GET /doc?url=`. This contract changes — the frontend
  would load by ID instead: `GET /doc?id=`.

- **Backward compatibility**: existing articles on disk are URL-keyed. Any new scheme
  needs a migration path or dual-lookup support.

- **WS `mark_synthesized` gap** (the issue that surfaced this design question):
  live WS sessions can't populate voice state because non-URL articles have
  no stable identity to write back to. UUID keys fix this — the client sends the
  article ID with the WS request, server updates `voices.json` on done.

## Open Questions

1. **Mutable text and audio cache invalidation**
   If the user edits a sentence, the cached audio for that sentence is stale.
   The audio cache key is SHA-256(normalized_text + voice_cache_key) — it will
   naturally miss on changed text. `voices.json` status moves to `stale` on any
   text PATCH; old audio remains playable. Per-sentence dirty state is more precise
   but complex. The future versioning vision (retaining original document) may change
   what "invalidation" means entirely. Not needed for Phase 1 — defer.

## Resolved

- **UUID for all documents** — UUID keys for everything, including URL-fetched; one-time migration script re-keys existing URL-slug directories and populates both indexes from existing frontmatter
- **Deduplication** — source_url index (primary) + content_hash index (redirect detection); hash taken once at fetch time and stored; `cached_at` gives author visibility into staleness
- **Original content saved** — originally fetched HTML stored as `source.html`; used for content hash; display to author deferred
- **Voice state** — `voices.json` per document replaces `synthesized_voices`/`voice_durations` frontmatter fields; four statuses: `in-progress`, `ready`, `stale`, `error`; stale is a warning badge, not a playback block; re-triggering an in-progress voice is a noop returning the existing job id
- **Voice picker** — populated from `voices.json`; author auditions voices (including stale), picks published voice, deletes unwanted jobs
- **Fetch flow** — `POST /documents` returns `{ id }` immediately; Phase 1 polls `GET /documents/:id`; Phase 2 replaces polling with WS `document_status` events
- **WS message protocol** — Phase 0: add `type` field to all WS messages; client console.logs and ignores any unrecognized types; makes Phase 2 WS events safe to add
- **API naming** — standardize on "document" throughout; `GET /documents/:id` replaces `GET /doc?url=`
- **Source URL vs. canonical URL** — cleanly separated: `id` is stable key, `source_url` is provenance metadata
- **Frontend WS identity** — client passes document ID with WS request; `voices.json` updated on done; creation must precede synthesis start
- **Snippet creation timing** — create document on first synthesis (author has shown intent); use first N words of text as provisional title
- **Everything is a document** — no special-casing for snippets, uploads, or pastes; author can delete; consistent with modern auto-save expectations
- **Upload / paste** — both supported; identity model works for both without special-casing
- **Concurrent index writes** — indexes kept in memory (`RwLock`); writes flush to disk via write-to-temp-then-rename; sentinel file triggers rebuild on next startup if flush fails
- **Phase ordering** — Phase 1 includes voice state so URL fetch → synthesize → listen loop is solid before expanding input surface area; Phase 3 adds PATCH, snippets, upload, paste
