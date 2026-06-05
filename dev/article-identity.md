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
- **PDFs / local files**: text extracted by external tools; may have a *provenance* URL
  (where the PDF came from) but that URL didn't produce the text — a different tool did
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

**Migration**: one-time throw-away script that re-keys existing URL-slug directories to UUID
and populates the source_url index. Manual run is fine (personal tool, small number of articles).

### Deduplication indexes

Two indexes in `~/.odoru/index/`:

- `source_url.json`: map of `url → uuid` — catches re-fetches of the same URL
- `content_hash.json`: map of `sha256(normalized body) → uuid` — catches redirects
  (URL A and URL B resolve to the same content)

On `POST /documents` with a URL: check source_url index first (cheap), then content_hash
(catches redirects). If found, return the existing document immediately — no fetch, no synthesis.

The `cached_at` frontmatter field gives the author visibility into when content was last fetched,
so they can judge whether a dedup hit is stale. A `POST /documents/:id/refresh` endpoint
can be added later if force re-fetch becomes a need.

Near-duplicate dedup (article updated slightly → same hash miss) is acceptable for a personal tool.

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

### Fetch flow (two phases)

**Phase 1 (UUID migration):**
- `POST /documents` with a URL body returns `{ id }` immediately
- Client polls `GET /documents/:id` until `status: ready`
- Unblocks authoring use cases and fixes `mark_synthesized` gap

**Phase 2 (WS events):**
- Replace polling with a `document_status` WS event: `{ id, status: "fetching" | "ready" | "error", ... }`
- Same channel already used for synthesis — fetch progress is one more event type
- Eliminates polling pattern; architecturally cleaner

## Known Constraints

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

- **`synthesized_voices` fast path** in `GET /doc`: currently keyed by voice ID in the
  frontmatter. Not affected by identity change.

- **WS `mark_synthesized` gap** (the issue that surfaced this design question):
  live WS sessions can't populate `synthesized_voices` because non-URL articles have
  no stable identity to write back to. UUID keys fix this — the client sends the
  article ID with the WS request, server calls `mark_synthesized` on done.

## Open Questions

1. **What triggers document creation for snippets?**
   On first WS synthesis? On explicit "save" action? On background job submission?
   An explicit save/create action is cleaner authoring UX but risks data loss if the
   author doesn't save before navigating away. Creating on first synthesis avoids loss
   but may create orphan documents for one-off experiments. Tension unresolved.

2. **Mutable text and audio cache invalidation**
   If the user edits a sentence, the cached audio for that sentence is stale.
   The audio cache key is SHA-256(normalized_text + voice_cache_key) — it will
   naturally miss on changed text. But `synthesized_voices` in frontmatter would
   be wrong (it claims all sentences are synthesized when some have changed).
   Need a strategy: clear `synthesized_voices` on any text edit? Track per-sentence
   dirty state? Note: future vision includes automatic versioning that retains the
   original document — this may change what "invalidation" means.

## Resolved

- **UUID for all documents** — UUID keys for everything, including URL-fetched; one-time migration script for existing URL-keyed articles
- **Deduplication** — source_url index (primary) + content_hash index (redirect detection); `cached_at` gives author visibility into staleness
- **Fetch flow** — `POST /documents` returns `{ id }` immediately; Phase 1 polls `GET /documents/:id`; Phase 2 replaces polling with WS `document_status` events
- **API naming** — standardize on "document" throughout; `GET /documents/:id` replaces `GET /doc?url=`
- **Source URL vs. canonical URL** — cleanly separated: `id` is stable key, `source_url` is provenance metadata
- **Frontend WS identity** — client passes document ID with WS request; `mark_synthesized` called on done; creation must precede synthesis start
- **PDF / file upload** — both upload and paste will be supported; identity model works for both without special-casing
