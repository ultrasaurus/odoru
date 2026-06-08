# Frontend

Frontend files are in `app/frontend/src/` (`*.ts` and `*.css`)

The logic is tightly coupled across these files:

- `app/frontend/src/main.ts` — view logic, DOM construction, player wiring
- `app/frontend/src/player.ts` — AudioContext, seek/highlight logic
- `app/frontend/src/ws.ts` — WebSocket connection, `sendSynth` / `cancelSynth`
- `app/frontend/src/document.ts` — `Document` class: fetch by URL, load by ID, WS status watch
- `app/frontend/src/markdown.ts` — markdown rendering, sentence span weaving
- `app/frontend/src/reader-core.ts` — outline rendering, shared between reader and export SPA
- `app/frontend/src/export-reader.ts` — export SPA entry point
- `app/frontend/src/types.ts` — shared TypeScript interfaces
- `app/frontend/src/style.css` — all styles; class names are shared across the above

Built with Vite + TypeScript, output to `app/frontend/dist/`.

## Reader view
- Pre-renders all sentences as gray `segment pending` spans immediately after doc fetch
- Player activates each span in place as audio arrives (removes `pending` class, wires click)
- Markdown rendered via `marked` — headings, paragraphs, bold/italic, blockquotes
- Sentence spans woven into markdown block elements; indices match server synthesis order
- Left sidebar: Documents tab + Outline tab (auto-selected on load)
  - Documents list driven by `GET /articles`, filtered to `publish: true`; clicking any item loads it
  - Defaults to first article in list; empty state shown if none published
  - On load, checks `GET /jobs` for an active job matching the article URL — auto-polls if found
- Outline tracks active heading from playback position; click → instant jump, no audio change
- "Synthesize in background" button shown when audio not cached and no active job exists
- Job progress shown in header; polls `GET /jobs/:id` every 4s while running
- Auto-scroll checkbox (default off) — when on, active sentence scrolls into view
- `viewCleanup` stops poll timers when navigating to Edit view

## Edit view
- URL fetch → markdown render → Synthesize (background job) → Listen / New buttons
- After fetch, article renders immediately as formatted markdown with gray pending sentence spans
- Synthesize starts a background job (`POST /jobs`); progress shown next to Synthesize button
- Listen: wires player to pre-rendered spans, opens WS synth session; play button enables on first segment
- New: resets to blank state (clears article, URL input, player)
- `loadAndListen(summary)` — called when a doc title is clicked in the Documents panel; loads doc
  by ID via `Document.load(id)`, renders markdown, calls `startListen()` immediately; works whether
  audio is cached, in-progress, or not yet started (WS streams whatever is available)
- Doc panel titles are always clickable (gold hover glow); clicking any doc opens it in the article area
- Voice picker ever-present in sidebar; user can synthesize the same doc with a second voice later
- Documents panel: always visible; fetches `GET /documents` + `GET /jobs` in parallel, polls every 10s
  - "hide ready" toggle in panel header collapses rows with no active/pending job; shows hidden count
  - One row per document; title click opens doc; status badge, progress bar, cancel, publish controls
  - Active jobs: progress bar + % + cancel button
  - Publish controls: checkbox + voice picker (voices with duration); changes fire `PATCH /documents/:id`
  - Metadata edit form (pencil button): title, author, date fields; toggled per-row
- `viewCleanup` stops all timers when navigating to Reader view
- Download enabled on `onSynthDone` (synthesis stream complete), not on playback end
- `downloadFilename()` evaluated at click time (lazy), not at view init

## Player timing model
- `AudioContext` plays segments as they arrive (streaming)
- `startTracking()` polls `AudioContext.currentTime` to update progress + highlighting
- `onSynthDone` fires when WS sends `{done: true}` — enables download
- `onEnded` fires when `done === true` AND playback position >= last segment end
- Seek: click transcript sentence → jump to that segment's start time; auto-resumes if playing
- `ws.onclose` handler: non-clean close fires `onError` so UI surfaces server crash
