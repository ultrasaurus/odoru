# Static Export

The authoring tool exports a self-contained single-page mini-site deployable to GitHub Pages or
opened as a local folder (`file://`). The reader becomes a standalone SPA with no server dependency.

## Command

```bash
dl spa <output-dir>
```

Reads from the document store and audio disk cache — no re-synthesis needed. All documents with
`publish: true` are included. Documents with a published voice and a complete audio cache are
exported with full audio playback; otherwise they are exported text-only (player disabled, warning
printed).

## Output structure

```
<output-dir>/
  index.html                ← fully self-contained SPA (JS + CSS inlined)
  documents/
    <slug>/
      audio/
        0000.mp3
        0001.mp3
        ...
```

The slug is `{date}-{title}` derived at export time, independent of the store UUID.

All JS and CSS are inlined into `index.html` by the CLI so the file works on `file://` without
triggering CORS errors. Audio files are the only external resources, and browsers allow
`<audio src="...">` on `file://` origins.

## Data payload

Manifest, transcripts, sentence-block counts, and document content are serialized as one JSON
object (`serde_json::to_string` in `run_spa`, `cli/src/main.rs`) and injected into `index.html` as
a script tag before `</head>`:

```html
<script>window.__ODORU__ = {"manifest": [...], "transcripts": {...}, "sentence_blocks": {...}, "documents": {...}};</script>
```

Field names are snake_case, taken directly from the Rust structs being serialized
(`ManifestEntry`, `ExportTranscriptEntry` in `util/src/export.rs`) — `reader-export.ts`'s
`ManifestEntry`/`TranscriptEntry`/`OdoruExport` TypeScript interfaces mirror this JSON shape
exactly and must be kept in sync with the Rust side by hand; there's no shared schema generation.

- `manifest` — array of `{ slug, title, authors, date, description, source_url, has_audio }`
- `transcripts` — map of slug → `[{ index, text, markdown_text, start, end, paragraph_end }, ...]`;
  timing is accumulated from per-sentence MP3 durations; `start`/`end` are `0.0` for text-only
  documents. `markdown_text` is the sentence's raw markdown (inline formatting intact) — falls
  back to `text` for a block where the markdown-aware split couldn't be aligned sentence-for-
  sentence with the plain-text split.
- `sentence_blocks` — map of slug → array of per-block sentence counts, in the same document-walk
  order `marked.lexer` produces client-side (one entry per heading/paragraph/list-item block).
  Lets the export weave sentence spans without re-deriving block boundaries.
- `documents` — map of slug → `{ content, plain_text }` (markdown + plain text for sentence
  splitting)

Both `markdown_text` and `sentence_blocks` are computed once at export time, in Rust, by
`tts::markdown::split_for_export` (`tts/src/markdown.rs`) — it reuses the same `pulldown_cmark`
block walk as `to_plain_text` and the same `splitter::split()` the server already calls, just also
keeping each block's raw markdown source (via byte offsets) alongside its plain text. This is what
lets the export render formatted sentence spans without ever loading the wasm splitter — see
"Shared code boundary" below.

## Frontend

`reader-export.ts` is a separate Vite entry point (`reader-export.html`) inside
`app/frontend/`. It shares:
- `markdown.ts` — `renderMarkdownFromEntries`, the shared `renderToken` block walk, sentence span
  creation. This module has no wasm dependency.
- `reader-core.ts` — `ReaderCore` (span highlighting, outline, click-to-seek), `formatByline`
- `style.css` — all reader styles

It does **not** share `markdown-live.ts`, which holds the wasm-backed `renderMarkdown` used by the
live app (`edit.ts`, `reader-author.ts`). Keeping that import out of `reader-export.ts`'s module
graph is what keeps the wasm asset (~98KB) out of the export's JS bundle — necessary because the
export must work when opened via `file://`, where a `fetch()` of the `.wasm` asset fails.

The export reader does not share the `Player` class or WebSocket layer — it has its own simpler
player that plays pre-built MP3 files directly.

## Audio playback: sliding window prefetch

Per-sentence MP3 files make seeking clean: seek = jump to sentence N, play that file.

- Window of 15 sentences buffered ahead of the current position
- On seek: `AbortController` is reset, a fresh window starts from the seek target
- Sentences already created as `Audio` objects are reused; only the window offset resets
- Click-to-seek on sentence spans and outline items both use the same seek path

## Shared code boundary (`reader-core.ts`, `markdown.ts`)

**IMPORTANT:** `reader-core.ts` is used by both the authoring app (`main.ts`) and the export
reader (`reader-export.ts`). Changes to it — or to anything it calls (`.segment`/`.pending`/
`.active` CSS rules) — must be verified in both paths.

`markdown.ts` itself is split along the same boundary:
- **Shared** (`markdown.ts`): `renderToken`'s block walk, silent-text handling, the
  `SentenceProvider` interface, and `renderMarkdownFromEntries` (export). No wasm dependency —
  changes here affect both paths and must be verified in both.
- **Live-app only** (`markdown-live.ts`): `renderMarkdown`, the wasm-backed `SentenceProvider`
  that calls into `splitter-wasm`. Only `edit.ts`/`reader-author.ts` import this module. Changes
  here do not affect the export and don't need export-path verification — but also don't fix
  export-side splitting bugs; see the data payload section above for where the export gets its
  sentence data instead.
