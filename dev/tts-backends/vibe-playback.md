# Design: player support for out-of-order segment arrival

Companion to `vibe/dev/odoru-import-prep.md` and `dev/tts-backends/vibe-import.md`, which
design how vibe-synthesized audio gets imported into Odoru's cache. This
doc covers what `app/frontend/src/player.ts` needs to change to actually
*play* imported audio once it exists, given that import can leave
permanent (or long-lived) gaps in the middle of a document.

Status: design discussion, not yet implemented. Not blocking `dl import`
shipping — the cache-population side and the playback side are separable.

## Motivation

`dl import` is "skip-and-report": a sentence whose transcript doesn't
parse, or that finds no word match, is skipped rather than aborting the
whole import. A partially-imported document ends up with some sentences
cached and others not — and unlike live synthesis, where every sentence
eventually arrives (or the whole stream errors out), an imported gap can
stay missing for a long time: at first while debugging the importer,
later because someone needs to manually re-run `dl import --segment <n>`
to backfill it. The intent is that the gap **will** eventually fill in —
just possibly minutes (or longer) later, not seconds.

The player's current design can't represent that. It assumes segments
arrive strictly in index order and uses that to assign each one a global,
cumulative position in the document's timeline.

## Current model (for reference)

- `Player.segments` is a plain array, built via `push()` as each WS
  message arrives.
- Each entry's `startTime`/`endTime` are cumulative: `startTime = prev
  ? prev.endTime : 0`. This only works because arrival order and array
  order are guaranteed to match.
- `seekTo(index)`: if `index < segments.length`, seek immediately. If not
  yet arrived, park (`pendingSeekIndex`) and fire `onWaitingCb`; resolves
  when a later WS message brings `segments.length` up past `index`.
- DOM segment spans (`renderSegment`) are activated — `pending` class
  removed, click handler attached — the moment that sentence's own audio
  arrives, independent of anything else. This part already generalizes
  fine to the new model; see below.

This breaks if a sentence is permanently (or long-term) missing while
later sentences keep arriving: `push()` would put a later sentence's audio
at the missing one's array slot, shifting every subsequent index down by
one and corrupting `highlightSegment`, `seekTo`, and the annotation
`segmentIndexForEl` lookups for the rest of the document.

## New model

- **`segments` becomes index-addressable**, not append-only. Fill in
  `segments[i]` by its real sentence index as audio for sentence *i*
  arrives, regardless of receive order. A not-yet-arrived index is simply
  absent (`undefined`), not a placeholder.
- **Track a "contiguous-known-through" pointer** separately from how much
  data has arrived. `startTime`/`endTime` for index *i* depend on every
  index before it being known (cumulative duration) — so only the
  unbroken run from 0 has a defined absolute position. A sentence that
  arrived out of order, past a gap, sits in the array with known *duration*
  but no absolute position yet.
- **No silence placeholder needed.** The earlier idea (synthesize actual
  silence for a missing sentence to keep `push()` simple) is unnecessary —
  index-addressing solves the same problem more directly, and the
  "pause" behavior below gives the desired UX without it.

## Playback semantics: natural progression vs. explicit seek

These are deliberately different:

- **Natural forward playback reaching a gap pauses.** The `AudioQueue`
  only ever gets fed the contiguous-known-through prefix. When playback
  reaches the end of what's enqueued and the document isn't fully
  resolved, that's exactly today's "ran out of synthesized audio, wait"
  state (`onWaitingCb`) — just triggered by hitting a gap during ordinary
  playback, not only by an explicit `seekTo` past the end of what's
  arrived so far. No new pause mechanism, just a new trigger for the
  existing one.
- **An explicit seek past a gap, to an already-arrived later sentence,
  should work immediately** — the user chose to skip it, which is
  different from playback running into a wall on its own. This needs
  `_doSeek` to stop requiring a global absolute position: treat the
  contiguous run starting at the seek target as its own local timeline
  (its own zero point), using that sentence's own known duration, rather
  than insisting on knowing every preceding sentence's duration first.
- **When a gap eventually closes**, splice the two runs together —
  recompute the back-run's absolute positions now that the previously-
  missing duration is known, so scrubbing back across the seam lines up
  correctly. Until that happens, total `duration` and the scrubber need to
  show an estimate (or "unknown") for anything past an unresolved gap.

## DOM segment activation already generalizes

`renderSegment`'s per-sentence "pending → ready" activation is already
keyed off "this sentence's own audio arrived," not off play-order or
contiguity. A sentence past a gap becomes clickable/highlighted the
instant its own audio shows up, identical to today's behavior — this part
needs no change beyond making sure the index passed in is the sentence's
real position, not an arrival-order counter.

## Other implications

- **`done` changes meaning.** Today `done: true` means "the WS stream
  ended, nothing more is coming." With long-lived gaps, `done` for *this
  session* doesn't mean a gap won't resolve later via a separate backfill
  (e.g. someone re-running `dl import --segment <n>`). Need a way to
  distinguish "this stream is finished" from "this document is fully
  resolved" — likely a separate signal/event for "a previously-missing
  segment just became available," independent of the original stream's
  `done`.
- **Existing latent bug, more likely to bite here:** `seekTo`'s
  "park and wait" path has no timeout and is never cleared on `done`. For
  live synthesis this is harmless (gaps are always seconds, not minutes).
  With import-driven gaps potentially lasting minutes or longer, a user
  who seeks/listens into a still-pending sentence and walks away could be
  stuck in "waiting" indefinitely. Should clear `pendingSeekIndex` (and
  surface something to the UI) when a session-level `done`/no-further-
  arrivals signal fires while a seek is still parked on a sentence
  confirmed not coming in this pass.
- **Backend gap (see `dev/tts-backends/vibe-import.md`'s "Known gap, deliberately
  deferred"):** none of this matters until playback can find imported
  audio at all. There's no `Voice::Imported` variant in `tts/src/backend.rs`
  yet — this is a new `Voice`/backend, not just a cache-key fix, and that
  backend has no real synthesis fallback for a true cache miss (vibe
  synthesis isn't real-time). This doc assumes that gap is closed
  separately; the out-of-order arrival problem exists either way once it
  is.

## Open items

- Exact mechanism for "a previously-missing segment just became
  available" — a new WS message type? A poll? Depends on how/where the
  backfill (`dl import --segment <n>`) actually runs relative to a live
  Odoru server process.
- How `duration`/scrubber UI should represent an unresolved gap's unknown
  contribution to total length — flat estimate per sentence? Hide the
  scrubber past the first gap? Not designed yet.
- Whether `_doSeek`'s "local timeline" treatment should also apply to
  *backward* seeks into an already-played run before a gap, or only
  forward past one — likely symmetric, not yet thought through.
