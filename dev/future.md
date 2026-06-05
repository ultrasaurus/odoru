# Future Design Notes

## Static Export / GitHub Pages Reader

### Overview

The authoring tool (client-server) will export a static single-page mini-site deployable to GitHub Pages. The reader view becomes a standalone SPA with no server dependency.

### Directory structure

```
export/
  index.html              ← SPA shell, routes by ?article=<slug> or hash
  manifest.json           ← article list (titles, slugs, descriptions)
  assets/
    index.js
    style.css
  articles/
    <slug>/
      meta.json           ← title, authors, date, url, voice
      transcript.json     ← [{index, text, start, end, paragraph_end}, ...]
      audio/
        0000.mp3
        0001.mp3
        ...
```

Export reads from the existing article store + audio disk cache — no re-synthesis needed if already cached.

Only articles with `publish: true` and a `published_voice` set are included. The export uses `published_voice` to select which audio files to copy. Both fields live in `article.md` frontmatter.

### Audio playback: sliding window prefetch + AbortController on seek

Per-sentence audio files make seeking clean: seeking = jump to sentence N, play that file.

**Prefetch strategy: sliding window**
- Keep N sentences (e.g. 10–20) buffered ahead of the current position
- On seek (outline click, scrub, timestamp jump): cancel all in-flight prefetch requests via `AbortController`, start a fresh window from the seek target
- Sentences already fetched stay in memory/browser cache — only in-flight requests are cancelled
- This bounds server/CDN load while keeping seek latency to one round trip for the target sentence

**Why not prefetch everything at load:**
- Queuing 200+ requests means a seek to sentence 195 has to wait behind everything already in flight
- HTTP/2 multiplexing helps but doesn't eliminate the problem
- AbortController-on-seek cleanly solves it for any window size

**Works identically** for the authoring server once synthesis is cached to disk.
