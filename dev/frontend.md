# Frontend

Frontend files are in `app/frontend/src/` (`*.ts` and `*.css`)

The logic is tightly coupled across these files:

- `app/frontend/src/main.ts` — view logic, DOM construction, player wiring
- `app/frontend/src/player.ts` — AudioContext, WebSocket, seek/highlight logic
- `app/frontend/src/markdown.ts` — markdown rendering, sentence span weaving
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
- `viewCleanup` stops poll timers when navigating to New view

## New view
- URL fetch + text area + voice picker + "Synthesize in background" checkbox
- Checkbox unchecked: live streaming WS synthesis
- Checkbox checked: `POST /jobs`, progress shown in transcript area, polls every 4s
- Documents panel below card: always visible; fetches `GET /articles` + `GET /jobs` in parallel,
  polls every 10s; one row per article with job status overlaid where `article_url` matches
  - Job matching restricted to `ARTICLE_VOICE` (reader hardcoded to f5:sarah for now)
  - Status: running → pending → job-done → synthesized-only → unsynthesized; newest `cached_at` first
  - Active jobs: progress bar + % + cancel button; ready: sentence count
  - Publish controls shown when `synthesized_voices` non-empty: checkbox + voice picker;
    changes fire `PATCH /doc?url=` immediately
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
