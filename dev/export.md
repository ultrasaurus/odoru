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

Manifest, transcripts, and document content are injected into `index.html` as a script tag before
`</head>`:

```html
<script>window.__ODORU__ = { manifest, transcripts, documents };</script>
```

- `manifest` — array of `{ slug, title, authors, date, description, source_url, has_audio }`
- `transcripts` — map of slug → `[{ index, text, start, end, paragraph_end }, ...]`; timing is
  accumulated from per-sentence MP3 durations; `start`/`end` are `0.0` for text-only documents
- `documents` — map of slug → `{ content, plain_text }` (markdown + plain text for sentence
  splitting)

## Frontend

`export-reader.ts` is a separate Vite entry point (`export-reader.html`) inside
`app/frontend/`. It shares:
- `markdown.ts` — `renderMarkdown`, sentence span creation
- `reader-core.ts` — `ReaderCore` (span highlighting, outline, click-to-seek), `formatByline`
- `style.css` — all reader styles

The export reader does not share the `Player` class or WebSocket layer — it has its own simpler
player that plays pre-built MP3 files directly.

## Audio playback: sliding window prefetch

Per-sentence MP3 files make seeking clean: seek = jump to sentence N, play that file.

- Window of 15 sentences buffered ahead of the current position
- On seek: `AbortController` is reset, a fresh window starts from the seek target
- Sentences already created as `Audio` objects are reused; only the window offset resets
- Click-to-seek on sentence spans and outline items both use the same seek path

## Shared code boundary (`reader-core.ts`)

**IMPORTANT:** `reader-core.ts` is used by both the authoring app (`main.ts`) and the export
reader (`export-reader.ts`). Changes to it — or to anything it calls (`markdown.ts`,
`.segment`/`.pending`/`.active` CSS rules) — must be verified in both paths.
