# Frontend

Frontend files are in `app/frontend/src/` (`*.ts` and `*.css`)

The logic is tightly coupled across these files:

- `app/frontend/src/main.ts` — boot: mounts reader or edit view, owns the cleanup handoff
- `app/frontend/src/edit.ts` — edit/synthesize view; exports `mount(onReader) → cleanup`
- `app/frontend/src/reader-author.ts` — authoring reader view; exports `mount(onEdit) → cleanup`
- `app/frontend/src/reader-core.ts` — outline rendering, shared between authoring reader and export SPA
- `app/frontend/src/reader-export.ts` — export SPA entry point
- `app/frontend/src/ui.ts` — shared types (`VoiceInfo`, `JobInfo`), helpers (`fmt`, `wireControls`, etc.)
- `app/frontend/src/jobs.ts` — `pollJob(jobId, total, callbacks) → stop`: polls `GET /jobs/:id` every 4s, calls onProgress/onDone/onError, retries silently on network error
- `app/frontend/src/player.ts` — AudioContext, seek/highlight logic
- `app/frontend/src/ws.ts` — WebSocket connection, `sendSynth` / `cancelSynth`
- `app/frontend/src/document.ts` — `Document` class: fetch by URL, load by ID, WS status watch
- `app/frontend/src/markdown.ts` — markdown rendering, sentence span weaving
- `app/frontend/src/types.ts` — shared TypeScript interfaces
- `app/frontend/src/style.css` — all styles; class names are shared across the above

View navigation uses a `mount() → cleanup` pattern: each view module exports a `mount` function
that sets up the DOM and returns a cleanup function. `main.ts` calls cleanup before mounting the
next view, so timers and audio always stop on navigation.

Built with Vite + TypeScript, output to `app/frontend/dist/`.

## Reader view
- Pre-renders all sentences as gray `segment pending` spans immediately after doc fetch
- Player activates each span in place as audio arrives (removes `pending` class, wires click)
- Markdown rendered via `marked` — headings, paragraphs, bold/italic, blockquotes
- Sentence spans woven into markdown block elements; indices match server synthesis order
- Left sidebar: Documents tab + Outline tab (auto-selected on load)
  - Documents list driven by `GET /documents`, filtered to `publish: true`; clicking any item loads it
  - Defaults to first document in list; empty state shown if none published
  - On load, checks for an active job on the document — auto-polls via `pollJob` if found
- Outline tracks active heading from playback position; click → instant jump, no audio change
- "Synthesize in background" button shown when audio not cached and no active job exists
- Job progress shown in header; polls `GET /jobs/:id` every 4s via `pollJob` utility
- Auto-scroll checkbox (default on) — when on, active sentence scrolls into view
- `cleanup()` (returned by `mount`) stops poll timers and audio when navigating to Edit view

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
- `cleanup()` (returned by `mount`) stops all timers and audio when navigating to Reader view
- Download enabled on `onSynthDone` (all audio received over WS), not on playback end
- `downloadFilename()` evaluated at click time (lazy), not at view init

## Player timing model
- `AudioContext` plays segments as they arrive (streaming)
- `startTracking()` polls `AudioContext.currentTime` to update progress + highlighting
- `onSynthDone` fires when WS sends `{done: true}` — enables download
- `onEnded` fires when `done === true` AND playback position >= last segment end
- Seek: click transcript sentence → jump to that segment's start time; auto-resumes if playing
- `ws.onclose` handler: non-clean close fires `onError` so UI surfaces server crash

## Stale-audio defence (two-layer)

Switching documents while audio is streaming requires stopping both the server-side stream
and any in-flight async work already queued in the player. Two guards work together:

**Layer 1 — `stream_id` filtering in `ws.ts`**
- Every `synth` request the server acknowledges with `synth_started { stream_id }` before
  sending any segments. The client stores `currentStreamId`.
- `sendSynth()` sends `cancel { stream_id }` to abort the previous server task, then clears
  `currentStreamId` (it will be set again when the new `synth_started` arrives).
- All incoming segment / done / error frames are dropped unless `msg.stream_id === currentStreamId`.
- This stops *new* frames from reaching the player after a document switch.

**Layer 2 — generation counter in `player.ts`**
- `player.reset()` increments `this.generation`. Each `synthesize()` call captures the value
  in `gen = this.generation`.
- After every `await decodeAudioData()` inside the decode chain, the code checks
  `this.generation !== gen` and discards the result if they differ.
- This catches segments that were already queued in `decodeChain` *before* `cancel` was sent —
  stream_id filtering stops frames at the WS boundary but can't reach work already in flight.

**Why both are needed**: stream_id covers the wire; the generation counter covers the async
decode gap. Remove either layer and a document switch can corrupt the new session's AudioQueue.

## `decodeChain` — serial segment processing

Segments are decoded in order via a Promise chain (`this.decodeChain`):
```ts
this.decodeChain = this.decodeChain.then(async () => {
  const samples = await decodeAudioData(...)
  if (this.generation !== gen) return  // stale — discard
  this.queue.enqueue(samples)
})
```
- Each `.then()` appends to the previous promise, so decodes are serial even when segments
  arrive out of order or faster than decode completes.
- **Do not parallelise**: audio must be enqueued in arrival order or playback corrupts.
- The generation check *must* be after the await, not before, because `reset()` can fire
  while `decodeAudioData` is running.

## `loadAndListen` and `loadSeq`

`loadAndListen(summary)` in the listen view is async (fetches full doc). Rapid document
switches can create races where a slow load completes after a newer one. `loadSeq` is a
module-level counter; each call captures `++loadSeq`. After `await Document.load()`, the
function returns early if `loadSeq` has since incremented. This prevents a stale doc from
overwriting the current one.

## Play button icon state

The play icon (`▶` / `⏸`) is updated in three places:
1. `loadAndListen` — resets to `▶` immediately so the icon is correct before audio arrives.
2. `onTimeUpdate` callback — updates on every animation frame while audio is playing.
3. `playBtn` click handler — updates after `player.toggle()` resolves.

If only (2) and (3) were present, switching documents while playing would leave a stale `⏸`
until the first `timeUpdate` fires for the new stream.
