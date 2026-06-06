# Static Export Design

The authoring tool (client-server) exports a static single-page mini-site deployable to GitHub Pages. The reader view becomes a standalone SPA with no server dependency.

## Directory structure

```
export/
  index.html              ← SPA shell, routes by ?doc=<slug> or hash
  manifest.json           ← document list (titles, slugs, descriptions)
  assets/
    index.js
    style.css
  documents/
    <slug>/               ← title-derived slug, generated at export time
      meta.json           ← title, authors, date, source_url, voice
      transcript.json     ← [{index, text, start, end, paragraph_end}, ...]
      audio/
        0000.mp3
        0001.mp3
        ...
```

Export reads from the document store + audio disk cache — no re-synthesis needed if already cached.

Only documents with `publish: true` and `published_voice` set are included. Export uses `published_voice` to select which audio files to copy.

Note: export slug is title-derived at export time, independent of the store key (UUID). See `overview.md`.

## Local static folder

The export is designed to work opened as a local folder (`file://`) as well as hosted on GitHub Pages. Audio plays fine via `<audio src="...">` on `file://`. Note: browsers block `fetch()` on `file://` origins — the SPA must load `manifest.json` and `transcript.json` via `<audio>` or must use a small local server; alternatively, inline the manifest and transcripts into the HTML/JS bundle at export time to avoid any `fetch()` calls.

## Audio playback: sliding window prefetch + AbortController on seek

Per-sentence audio files make seeking clean: seeking = jump to sentence N, play that file.

**Prefetch strategy: sliding window**
- Keep N sentences (e.g. 10–20) buffered ahead of the current position
- On seek (outline click, scrub, timestamp jump): cancel all in-flight prefetch requests via `AbortController`, start a fresh window from the seek target
- Sentences already fetched stay in memory/browser cache — only in-flight requests are cancelled
- This bounds CDN load while keeping seek latency to one round trip for the target sentence

**Why not prefetch everything at load:**
- Queuing 200+ requests means a seek to sentence 195 has to wait behind everything already in flight
- HTTP/2 multiplexing helps but doesn't eliminate the problem
- AbortController-on-seek cleanly solves it for any window size