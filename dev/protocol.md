# API Protocol

## REST API (`app/src/main.rs`)
```
GET  /voices               → { voices: [{id, name, backend, description}] }

POST /documents            ← { url } → { id }   (returns immediately; fetch runs async)
GET  /documents            → [{ id, status, source_url?, title?, authors, date?,
                                description?, cached_at?, publish, voices }]
GET  /documents/:id        → { id, status, source_url?, title?, authors, date?,
                                description?, cached_at?, content?, plain_text?,
                                publish, voices, error? }
PATCH /documents/:id       ← { publish?: bool, published_voice?: string } → 204

GET  /ws                   → WebSocket upgrade
POST /jobs                 ← { text, voice, document_id? } → job (deduplicates by text+voice)
GET  /jobs                 → [job, ...]
GET  /jobs/:id             → job
DELETE /jobs/:id           → cancel job
```

### Document status (`GET /documents/:id`)
- `status: "fetching"` — fetch in progress; `content`, `plain_text`, `source_url` are `null`
- `status: "ready"` — all fields populated
- `status: "error"` — fetch failed; `error` field contains the message
- Poll until `status: "ready"` (Phase 2 will replace polling with WS events)

### Voice state shape (in document responses)
```json
{
  "f5:sarah": { "status": "ready", "duration": 312.4, "job_id": "...", "published": true },
  "f5:nova":  { "status": "in-progress", "job_id": "..." }
}
```
- `status`: `in-progress | ready | stale | error`
- `stale`: content changed since synthesis — old audio still playable, warning badge shown
- `published: true` on at most one voice per document
- `publish` in document frontmatter: document-level intent; `false` overrides any `published` voice

### Deduplication on `POST /documents`
1. Check `source_url` index (fast path — same URL)
2. Check `content_hash` index (catches redirects — same content from different URL)
3. On miss: create `fetching` record, return `{ id }`, spawn async fetch

## WebSocket messages

Client → server (voice must be prefixed, document_id optional):
```json
{ "text": "...", "voice": "f5:sarah", "document_id": "uuid" }
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

## Pending spans contract

The client pre-renders all sentences as gray `.segment.pending` spans before synthesis
begins. As each WebSocket segment arrives, the player activates the span at that index
in place. This requires the client's sentence order to match the server's exactly.

**Server side** (`tts/src/splitter.rs`):
- Splits `plain_text` into sentences using `unicode_segmentation::unicode_sentences()`
- Paragraphs separated by `\n\n`; single `\n` is a hard break within a paragraph
- Abbreviations protected before splitting: `Mr`, `Dr`, `e.g`, `i.e`, etc. (see `ABBREVS` in splitter.rs)

**Client side** (`app/frontend/src/markdown.ts`):
- Splits `plain_text` using `Intl.Segmenter` with the same abbreviation protection
- Renders `content` (trafilatura markdown) into block elements (`<h1>`, `<p>`, `<blockquote>`, etc.)
- Weaves one `.segment.pending` span per sentence into each block element
- `plain_text` is the source of truth for sentence indices — not the markdown content field

**Critical**: any divergence between server and client sentence splitting causes spans to
activate out of sync with audio. If you change `ABBREVS` or splitting logic on either side,
update the other to match.
