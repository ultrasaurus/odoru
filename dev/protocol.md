# API Protocol

## REST API (`app/src/main.rs`)
```
GET  /voices          → { voices: [{id, name, backend, description}] }
GET   /doc?url=&voice= → { url, title, authors, date, plain_text, content,
                            synthesized_voices: [voice_id, ...],
                            cached: { content: bool, audio: voice_cache_key|null },
                            audio_duration_secs?: number,
                            publish: bool, published_voice?: string }
PATCH /doc?url=        ← { publish: bool, published_voice?: string } → 204
GET  /articles         → [{ url, title?, authors, date?, description?, cached_at,
                             synthesized_voices, voice_durations,
                             publish: bool, published_voice?: string }]
GET  /ws              → WebSocket upgrade
POST /jobs            → { text, voice, url? } → job (deduplicates by text+voice)
GET  /jobs            → [job, ...]
GET  /jobs/:id        → job
DELETE /jobs/:id      → cancel job
```

## WebSocket messages

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
