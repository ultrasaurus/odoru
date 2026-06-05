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

## Design Ideas

### Stable UUID key

Assign a UUID to each article at creation time. The store directory becomes
`~/.odoru/articles/<uuid>/` rather than `~/.odoru/articles/<url-slug>/`.

- Identity is completely decoupled from content and source
- Text, title, source URL are all mutable metadata
- Jobs reference the article by UUID instead of URL
- `GET /articles` returns UUID alongside other fields
- Export slug = UUID (or a title-derived slug generated at export time)

The `source_url` field in frontmatter becomes optional provenance metadata —
"where this content originally came from" — not an identity field.

**Migration**: existing URL-keyed articles need to either be re-keyed (migration script)
or the store needs to support both formats during a transition period. Since it's a
personal tool with a small number of articles, a one-time migration is feasible.

### Hybrid: keep URL key for URL-fetched, UUID for everything else

URL-fetched articles keep their current key scheme (backward compatible).
New non-URL articles get a UUID key. The store distinguishes by presence of `source_url`.

Simpler migration (none needed for existing articles), but two code paths to maintain.
Lookup becomes slightly more complex.

### Title-slug key (generated lazily)

Generate a slug from the title once one exists. Before a title is set, use a UUID.
The key can be "promoted" from UUID to title-slug when a title is confirmed.

More human-readable on disk, but mutable keys create rename complexity.
Probably not worth it.

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

1. **UUID or URL slug for URL-fetched articles going forward?**
   Migrate existing URL-keyed articles to UUID, or keep URL as key for URL-sourced
   content and UUID only for new content types? The hybrid approach avoids migration
   but adds permanent complexity.

2. **What triggers article creation for snippets?**
   On first WS synthesis? On explicit "save" action? On background job submission?
   Creating on first synthesis is convenient but may create orphan articles for
   one-off experiments. An explicit save/create action is cleaner authoring UX.

3. **How does the frontend identify "current article" during a WS session?**
   Today it doesn't need to — WS is stateless per-message. With mark_synthesized,
   the client needs to pass an article ID. This means article creation must happen
   *before* or *at the start of* synthesis, not after.

4. **Mutable text and audio cache invalidation**
   If the user edits a sentence, the cached audio for that sentence is stale.
   The audio cache key is SHA-256(normalized_text + voice_cache_key) — it will
   naturally miss on changed text. But `synthesized_voices` in frontmatter would
   be wrong (it claims all sentences are synthesized when some have changed).
   Need a strategy: clear `synthesized_voices` on any text edit? Track per-sentence
   dirty state?

5. **Source URL vs. canonical URL**
   For URL-fetched articles, the current `url` field serves double duty: it's both
   the fetch key and the provenance. With UUID keys these separate cleanly:
   `id` = stable key, `source_url` = where it came from. But `GET /doc` today
   fetches-or-returns by URL — the "fetch from web" trigger needs a new home
   (maybe `POST /articles` with a URL body, returning the new article).

6. **PDF / file upload**
   How does extracted text arrive at the server? Direct text paste (current New view)
   is one path. A file upload endpoint is another. The identity model should work for
   both without special-casing.
