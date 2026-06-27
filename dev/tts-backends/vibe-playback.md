# Design: player support for sentences with no audio (imported-voice gaps)

Companion to `vibe/dev/odoru-import-prep.md` and `dev/tts-backends/vibe-import.md`,
which design how vibe-synthesized audio gets imported into Odoru's cache.
This doc covers what `app/frontend/src/player.ts` (and a small server-side
piece in `app/src/main.rs`) needed for imported audio to actually *play*,
given that import can leave gaps in the middle of a document.

Status: implemented.

## Motivation

`dl import` is "skip-and-report": a sentence whose transcript doesn't
parse, or that finds no word match, is skipped rather than aborting the
whole import. A partially-imported document ends up with some sentences
cached and others not.

The player's original design couldn't represent that. It assumed segments
arrive strictly in array order and used `push()` to build `Player.segments`,
assigning each one a cumulative timeline position purely by arrival order.
A gap shifted every subsequent sentence's array position down by one,
corrupting `highlightSegment`, `seekTo`, and the annotation
`segmentIndexForEl` lookups for the rest of the document.

**Not specific to imported audio, in hindsight:** `engine.rs`'s live
synthesis loop already skips a blank or symbol-only "sentence" (e.g. a
stray markdown artifact) without emitting anything for that index — so this
bug was reachable before any import work existed, just rare enough not to
have been noticed. Imported voices made gaps common enough to surface it.

## What shipped

**Backend (`app/src/main.rs`):** imported voices don't get a new
`tts::backend::Voice` variant — there's no live model to synthesize with,
so there'd be nothing for it to do on a cache miss. Instead `is_imported_voice`
keys off the `voice_id` prefix (`backend != "f5" && backend != "kokoro"`)
and routes to `handle_synth_replay`, a replay-only path that never touches
a `TtsBackend`: it walks the document's sentences, looks up each one's
audio in `audio_cache` under the per-document-and-index-scoped key (see
`vibe-import.md`), and streams whatever it finds. A sentence with no cache
entry just isn't sent — no explicit "gap" message (an earlier version of
this added one, `SegmentGapMsg`/`type: "gap"`; removed once the client
became index-aware, since `done` plus a still-missing index says the same
thing without a second protocol path to keep in sync).

**Client (`app/frontend/src/player.ts`):**

- `Player.segments` and `segmentEls` are filled in by the segment's real
  sentence index (`msg.index` from the WS frame), not by arrival order —
  `this.segments[msg.index] = ...` instead of `push()`. `pendingSpans[i]`
  (built up front from the rendered markdown, independent of audio) is the
  source of truth for which DOM span a given index belongs to.
- Arrivals are still strictly increasing in index (the server sends sentence
  0, then 1, then 2, etc., just silently skipping ones with no audio) —
  never genuinely out of order. That meant the full "local timeline
  splicing for out-of-order arrival" design once sketched here wasn't
  needed. `startTime`/`endTime` are still cumulative, chained off
  `segments[segments.length - 1]`, which remains safe because the highest
  assigned array index is always the most recent *real* arrival — gaps only
  ever leave holes at lower indices, never at the tail.
- A hole at a lower index is a real possibility for the lifetime of one
  synthesis session (between the gap being skipped and `done` arriving) —
  `highlightCurrent`'s scan, `_doSeek`'s enqueue loop, `downloadWav`, and
  `listenTo`'s bounds check all guard against an unset entry now.
- **On `done`**, `fillRemainingGaps()` backfills every still-missing index
  up to `pendingSpans.length` (the known total sentence count) with a
  zero-duration placeholder (`samples: new Float32Array(0)`, `startTime ===
  endTime`). After this, `segments` is dense again — no consumer needs
  gap-awareness of its own beyond the guards above. `_doSeek` skips
  zero-length samples, so seeking onto a gap just continues to whatever
  real audio follows it.
- **`seekTo`'s "park and wait" no longer hangs forever.** It used to only
  resolve via an exact-arrival match; if the awaited index turned out to be
  a permanent gap, nothing would ever satisfy it. Now `onDone` checks
  `pendingSeekIndex` after backfilling gaps and resolves it unconditionally
  — either to real audio that arrived after the gap, or (if the index
  itself was the gap) to its zero-duration placeholder, which `_doSeek`
  skips past to whatever's next. Either way the seek completes instead of
  leaving the UI in "waiting" indefinitely.
- `onReadyCb` (fires once "enough to start" has arrived) switched from "the
  first segment received happens to be index 0" to "the first segment
  received, whatever its index" — the former silently never fired at all
  if sentence 0 itself was a gap.

## DOM segment activation needed no changes

`renderSegment`'s per-sentence "pending → ready" activation was already
keyed off "this sentence's own audio arrived," not arrival order or
contiguity — once the *caller* started passing the real index instead of
an arrival counter, this part worked correctly unmodified.

## Known follow-ups (not implemented)

- **No separate "permanently missing" visual state — and that's correct,
  not a gap to fill in.** A gap today just means "the manual
  `dl import --segment <n>` step hasn't been run yet," and that step is
  expected to become automatic eventually — there's no actual permanence to
  represent. Leaving the span in its ordinary gray `pending` look (nothing
  ever removes that class for a backfilled gap) already says the accurate
  thing: not available *yet*. Revisit only if a real "this will never have
  audio" case shows up later.
- **Backfilling a gap *after* a session has already gone through `done`.**
  This implementation resolves everything at `done` time, within one
  synthesis/replay session. It does not handle "someone ran
  `dl import --segment <n>` to fix a gap while a client already has the
  document open, with `done` already fired minutes or hours ago." That
  would need a live update path (new WS message, or a poll) distinct from
  the normal segment stream — not designed, since it wasn't needed for the
  case actually being fixed (a fresh load of an already-imported document
  with permanent gaps, not a live backfill against an open session).
- **`duration`/scrubber accuracy is no longer a concern this doc needs to
  track** — since gaps resolve within the same session (zero-duration
  placeholders, not unknowns), `get duration()` is exact by the time `done`
  fires. The "show an estimate for an unresolved gap" problem from the
  earlier draft of this doc doesn't apply under the model that shipped.
