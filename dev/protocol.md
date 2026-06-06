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
PATCH /documents/:id       ← { publish?: bool, published_voice?: string,
                               content?: string, plain_text?: string,
                               title?: string, authors?: string[], date?: string } → 204
DELETE /documents/:id      → 204   (cancels in-progress jobs first, then removes directory)

GET  /ws                   → WebSocket upgrade
POST /jobs                 ← { text, voice, document_id? } → job (deduplicates by text+voice)
GET  /jobs                 → [job, ...]
GET  /jobs/:id             → job
DELETE /jobs/:id           → cancel job
```

### PATCH /documents/:id fields
- `publish` + `published_voice`: set document publish intent and which voice is published
- `content` + `plain_text`: update document body; both must be provided together; marks all `ready`/`in_progress` voices `stale` (old audio remains playable with warning badge)
- `title`, `authors`, `date`: update metadata; any subset may be provided; `authors` is an array of strings

### Document status (`GET /documents/:id`)
- `status: "fetching"` — fetch in progress; `content`, `plain_text`, `source_url` are `null`
- `status: "ready"` — all fields populated
- `status: "error"` — fetch failed; `error` field contains the message
- Poll until `status: "ready"` (Phase 2 will replace polling with WS events)

### Voice state shape (in document responses)
```json
{
  "f5:sarah": { "status": "ready", "duration": 312.4, "job_id": "...", "published": true },
  "f5:nova":  { "status": "in_progress", "job_id": "..." }
}
```
- `status`: `in_progress | ready | stale | error`
- `stale`: content changed since synthesis — old audio still playable, warning badge shown
- `published: true` on at most one voice per document
- `publish` in document frontmatter: document-level intent; `false` overrides any `published` voice

### Deduplication on `POST /documents`
1. Check `source_url` index (fast path — same URL)
2. Check `content_hash` index (catches redirects — same content from different URL)
3. On miss: create `fetching` record, return `{ id }`, spawn async fetch

## WebSocket

One persistent connection per client, opened at startup and shared across views.

### Client → server
```json
{ "type": "synth", "text": "...", "voice": "f5:sarah", "document_id": "uuid" }
{ "type": "watch", "document_id": "uuid" }
```
- `synth`: synthesize text. `voice` must be prefixed (e.g. `"f5:sarah"`). `document_id` optional — if provided, `voices.json` is updated on completion.
- `watch`: subscribe to `document_status` events for a document. Send after `POST /documents` returns `{ id }`.

### Server → client
One per sentence (during synthesis) — **two frames**:

Frame 1 — JSON header (Text frame):
```json
{ "type": "segment", "index": 0, "transcript": {"start": 0.41, "end": 1.65, "text": "..."},
  "cached": bool, "paragraph_end": bool }
```
Frame 2 — raw audio (Binary frame): f32le PCM @ 24 kHz, no encoding.

Client must set `ws.binaryType = 'arraybuffer'`. On receiving a binary frame, pair it with
the most recent JSON header to reconstruct the segment (`new Float32Array(ev.data)`).
Synthesis complete:
```json
{ "type": "done" }
```
Synthesis error:
```json
{ "type": "error", "error": "..." }
```
Document fetch complete (sent to connections that sent `watch` for this id):
```json
{ "type": "document_status", "id": "uuid", "status": "ready", "title": "..." }
{ "type": "document_status", "id": "uuid", "status": "error", "error": "..." }
```
- Client ignores any `type` it doesn't recognize (safe to add new event types)
- `viewCleanup` stops audio (`player.stop()`) but keeps WS open — connection is a browser-session singleton

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
