# Frontend

All source files are in `app/frontend/src/`. Built with Vite + TypeScript; output to `app/frontend/dist/`.

Module layering — `main.ts` mounts one of the two views, both of which share a services
layer; `ws.ts` is isolated beneath it, reached only via `document.ts` and `player.ts`:

```
                main.ts
                /     \
       reader-author.ts  edit.ts
                \     /
        ┌────────┼────────┬─────────┐
        │        │        │         │
   document.ts player.ts jobs.ts markdown.ts
        \         /
          ws.ts
```

Files:

- `main.ts` — boot: mounts reader or edit view, owns the cleanup handoff
- `edit.ts` — edit/synthesize view; exports `mount(onReader) → cleanup`
- `reader-author.ts` — authoring reader view; exports `mount(onEdit) → cleanup`
- `reader-core.ts` — outline rendering, shared between authoring reader and export SPA
- `reader-export.ts` — export SPA entry point
- `ui.ts` — shared types (`VoiceInfo`, `JobInfo`), helpers (`fmt`, `wireControls`, etc.)
- `jobs.ts` — `pollJob(jobId, total, callbacks) → stop`: polls `GET /jobs/:id` every 4s, calls onProgress/onDone/onError, retries silently on network error
- `player.ts` — AudioContext, seek/highlight logic
- `ws.ts` — WebSocket connection, `sendSynth` / `cancelSynth`
- `document.ts` — `Document` class: fetch by URL, load by ID, WS status watch
- `markdown.ts` — markdown rendering, sentence span weaving
- `types.ts` — shared TypeScript interfaces
- `style.css` — all styles; class names are shared across the above

View navigation uses a `mount() → cleanup` pattern: each view module exports a `mount` function
that sets up the DOM and returns a cleanup function. `main.ts` calls cleanup before mounting the
next view, so timers and audio always stop on navigation.

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

## Two synthesis paths

The app intentionally keeps two separate synthesis paths, each suited to a different use case:

**WS streaming (`synth` message → segment frames)** — used by the Listen button and `loadAndListen`.
- Synthesizes sentences on demand; client can seek to any sentence and hear it within seconds.
- Server starts from the requested position, so seeking to sentence 80 doesn't wait for 1–79.
- Ephemeral: if the tab closes, the stream is lost. No persistence across sessions.
- Single synthesis slot per WS connection — starting a new stream cancels the previous one.

**Background job (`POST /jobs` → poll `GET /jobs/:id`)** — used by the Synthesize button.
- Synthesizes the full document in order, storing every segment in the audio cache.
- Survives tab close / server restart; progress is queryable at any time via REST.
- Once complete, all audio is cached — subsequent Listen sessions get instant cache hits.
- Multiple voices can be synthesized concurrently (separate jobs, one progress bar each).

**Why not consolidate?** Seeking into a not-yet-synthesized doc works well over WS because the
server can jump to the requested sentence immediately. A jobs-only path would require waiting for
all earlier sentences to be synthesized first, making mid-document seeks much slower.

## Edit view
- Two input modes, selected via **URL | Text** tabs:
  - **URL tab**: paste a URL, press Enter — fetches and extracts article via trafilatura; starts in Preview + Listen mode
  - **Text tab**: textarea for markdown; starts in Edit mode for new docs; existing docs start in Preview + Listen
- Always-visible fields below the tabs (both modes): **Title**, **Source URL**
  - Title and Source URL auto-save via 4s debounce (`PATCH /documents/:id`, metadata only, no re-synth)
  - **UUID**: shown as a small selectable label tucked into the bottom-right corner of the player card
    (`#doc-id-display`, absolutely positioned within `.card`), once doc is created
- **Edit/Preview/New buttons**: live in the `.input-tabs` row alongside URL/Text tabs — Edit/Preview
  to the left of New, New on the far right (`.input-tabs-spacer` pushes them right)
- **Edit / Preview toggle** (shown once a doc is loaded, replaces Synthesize):
  - **Edit**: textarea visible, article hidden, player stops
  - **Preview**: article visible with rendered spans; if content changed since last render, triggers re-synth (see below)
  - Clicking the Text tab while a doc is loaded also enters Edit mode
- **Auto-save** while in Edit mode: PATCH content only (no job) — on sentence-ending punctuation (`.?!`) or 4s debounce
- **Preview re-synth** (only when content changed): pause active jobs → PATCH content → WS stream → `POST /jobs`; see [editing.md](editing.md)
- `lastRenderedContent` guard: Edit → Preview with unchanged content keeps existing live spans and audio intact
- **Synthesize** button (shown for new text docs before first save): calls `setEditMode(false)`, triggering the same Preview re-synth flow
- New: resets to blank state (clears all fields, article, player)
- `loadAndListen(summary)` — called when a doc title is clicked in the Documents panel; loads doc
  by ID via `Document.load(id)`, renders markdown, populates title/source URL/textarea, calls `startListen()` immediately
  - Switches to URL tab if doc has a `source_url`; Text tab otherwise
  - Always starts in Preview mode; Edit/Listen/New buttons shown
  - Selects the doc's published voice (`voices[id].published === true`) if one exists, else falls back
    to the default-pick logic
- Doc panel titles are always clickable (gold hover glow); clicking any doc opens it in the article area
- Voice picker (sidebar): lists every voice from `/voices`, grouped by backend; each row shows a status
  badge (✓/⚙/~/✕) for the open document's `voices[id].status`, blank if never synthesized for this doc
  - `selectVoice(id, restartPlayer?)` — updates labels/description always; when `restartPlayer` is true
    (user clicked a voice row, or changed the queue's publish-voice picker for the open doc) and a doc
    with `plain_text` is loaded, stops the player and re-runs `synthesize()` with the new voice
  - user can synthesize the same doc with a second voice later
- Documents panel: always visible; fetches `GET /documents` + `GET /jobs` in parallel, polls every 5s
  - One row per document; title click opens doc; toggle arrow (visible on row hover, or when expanded) reveals details
  - Status badge only — shows `✓`/`⚙`/`⏸`/`✕`/etc; hidden entirely when the only state is "ready" (✓)
  - No progress bar or job controls in the queue rows — those live in the jobs panel (below)
  - Open (expanded) rows get a top/bottom border + raised background; closed rows have no border
  - Publish controls (in expanded body): checkbox + voice picker listing **all** voices in `voices.json`
    (not just those with `duration`), each with a status icon (✓/⚙/~/✕); changes fire `PATCH /documents/:id`
    and, if this doc is currently open in the editor, also call `selectVoice(..., restartPlayer: true)`
  - Metadata edit form (pencil button): title, author, date fields; toggled per-row
  - Panel is sticky, full viewport height (`100vh - header height`); only the row list scrolls
- Jobs panel (header, shown when any job is active): one row per document with an active job
  - Top row: title (click to open doc) + first active job's inline controls (voice name, progress
    bar, %, pause/resume, delete-voice via `DELETE /documents/:id/voices/:voice_id`)
  - If a doc has more than one active job, an expand toggle reveals the rest below
  - `pollJob` ([jobs.ts](../app/frontend/src/jobs.ts)) has a `paused` branch — when a watched job
    is paused, callers can show a resume affordance instead of treating it as an error
- `cleanup()` (returned by `mount`) stops all timers and audio when navigating to Reader view
- Download enabled on `onSynthDone` (all audio received over WS), not on playback end
- `downloadFilename()` evaluated at click time (lazy), not at view init

## Sticky full-viewport layout

Both the Documents panel (`.queue-column`) and the editor card (`.card-column`) use the same
sticky pattern so each fills the viewport below the header and scrolls only its inner content:

- A `--header-height` CSS variable is set on `<html>` from the header's `offsetHeight` (measured
  in JS, updated on `resize`); both columns use `height: calc(100vh - var(--header-height, 0px))`
  and `position: sticky; top: var(--header-height, 0px)`.
- `.queue-column` → `.queue-section` → `.queue-list` is a flex column; only `.queue-list` scrolls
  (`overflow-y: auto; flex: 1; min-height: 0`).
- `.card-column` → `.card` is a flex column; `.input-area` is the `flex: 1; min-height: 0` region,
  and within it `.article-area` scrolls (`overflow-y: auto`) while `.controls`/`.synth-row` stay
  fixed via `flex-shrink: 0`. This keeps the player pinned to the bottom of the viewport while only
  the article text scrolls.
- Below 280px width (matches the panel's min-width), both columns drop back to static/auto height
  so they stack normally on very small screens.

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
