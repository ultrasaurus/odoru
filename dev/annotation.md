# Annotations

Authors can select text (word-to-phrase granularity) and highlight it with a color,
like marking up a paper with highlighters. Independent of audio playback highlighting.

## Terminology

- **Annotation**: a colored highlight on a span of text, created by the author
- Distinct from the audio "highlight" (active sentence during playback)

## Scope

- Author Read view only (not the publish-preview / export SPA)
- Single-user for now; multi-user deferred (login coming later)

# Implementation

## Data model

```ts
interface Annotation {
  id: string        // base64-encoded UUID (~22 chars, easy to read while debugging)
  text: string      // literal matched string (re-match anchor)
  context: string   // ~40 chars before+after for disambiguation
  color: AnnotationColor
  created_at: string
}

type AnnotationColor = 'yellow' | 'coral' | 'mint' | 'blue' | 'lavender'
```

### Color palette (pastel highlighter set)

| Name     | Hex       |
|----------|-----------|
| Yellow   | `#fde68a` |
| Coral    | `#fca5a5` |
| Mint     | `#6ee7b7` |
| Blue     | `#93c5fd` |
| Lavender | `#c4b5fd` |

### Anchoring

Annotations are anchored by the **literal matched string** plus surrounding context,
not by position. On load, the rendered DOM is walked to find text matching
`annotation.text`; context disambiguates when the same phrase appears multiple times.

If the text is deleted, the annotation is silently dropped (optionally with a warning
when it transitions from matched → unmatched). Primary use case is highlighting a
reference for later use as a quote.

### Segment click handling

`Player.renderSegment` attaches a direct bubble-phase click listener to each
`.segment` span (triggers `seekTo`). Any delegated click handler added on an
ancestor (e.g. `articleContent`) for child elements like `.annotation` marks
fires *after* the segment's own listener in bubble phase — too late to
prevent `seekTo` from also firing on the same click.

Register delegated click handlers on ancestors of `.segment` with
`{ capture: true }` when they need to intercept/override a segment's own
click behavior, and call `e.stopPropagation()` inside. See
`app/frontend/src/edit.ts` (annotation click handler). Relevant to any future
feature adding another overlay element inside segments (inline footnotes,
comments, etc).

### Storage

Annotations persist to a server-side sidecar file, `annotations.json`,
alongside `voices.json` for each document — keyed by document UUID. This
makes future migration to per-user storage straightforward once auth lands
(see [future.md](future.md) Multiple Authors section).

### F5 alignment — implemented

F5's audio cache key is built from *normalized* text (numbers/abbreviations
expanded via `tts::f5::normalizer::normalize`), but annotations store the
*original* sentence text. `POST /voices/:voice_id/words` now works for F5 too
(previously returned 501) by mapping normalized-text word timestamps back to
original-text offsets — see Stage 3 below for the full design, and
[normalize.md](normalize.md)'s Implementation section for the span-mapping
mechanics it depends on.

Kokoro's `ensure_words` (`tts/src/alignment.rs`) needs none of this — Kokoro
uses raw sentence text as both the cache key and the alignment input, so its
aligned words are already original-text words. F5 has no such symmetry,
which is why it needed the extra mapping step.

## Key files

- `app/frontend/src/annotations.ts` — annotation logic: create, apply (with
  ambiguity-aware re-matching), delete, listen (`findAnnotationWordRange`)
- `app/frontend/src/edit.ts` — wires annotation picker and click-to-listen handler
- `app/frontend/src/player.ts` — `listenTo`, `segmentIndexForEl`, `stopAt`
- `app/frontend/src/style.css` — `.annotation` styles, loading/error states
- `util/src/documents.rs` — `read_annotations` / `write_annotations` sidecar helpers
- `util/src/normalizer.rs` — `normalize_with_spans` / `NormalizedText::source_range`:
  the span-mapping F5 alignment depends on (see [normalize.md](normalize.md))
- `tts/src/alignment.rs` — `ensure_words`: lazy forced alignment, cached in
  sidecar, per-key locked against concurrent duplicate alignment runs;
  `words_with_original_text`: maps F5's normalized-text aligned words back to
  original-text substrings (merging multiple normalized words that share one
  expanded source span into a single entry)
- `tts/src/audio_cache.rs` — `Meta` struct (now public, includes `words` field),
  `meta_path`, `mp3_path`, `read_meta`, `write_meta` helpers
- `tts/src/lib.rs` — exports `pub mod alignment`
- `app/src/main.rs` — REST endpoints: annotations CRUD + `POST /voices/:id/words`
  (Kokoro and F5 both handled), background `align_annotations_for_doc` task on
  annotation save
- `dev/annotation.md` — this file

# Plan

## UX — creating an annotation ✓ done

1. User selects text in Read mode via click-drag (normal browser selection)
2. On `mouseup`, if selection is non-empty and within `#article-area`: show a small
   color-picker popover near the selection with 5 color swatches
3. User clicks a color → annotation saved → popover closes → highlight applied in-place
4. Escape or click-away dismisses without saving

**Cross-sentence selections ✓ done:** a selection crossing `.segment` boundaries is
no longer trimmed to the anchor sentence. `wrapSelection` and `applyAnnotationToDOM`
both build one document-wide text (`buildDocPosition` walks every text node in the
container, not just `.segment`s, so it captures the literal space text node the
renderer inserts between sentences — matching `Range.toString()` exactly) and search
it as a single string. A match is wrapped by `wrapRange`, which walks the container's
text nodes directly (the same traversal `buildDocPosition` used to compute offsets)
and wraps whatever portion of the range falls in each one — segment-internal text,
the inter-segment gap, and a segment with multiple text nodes (inline formatting) are
all handled the same way, each fragment getting its own `<mark>` sharing the
annotation's `id`/color so CSS renders the whole thing as one continuous highlight.

An earlier version split by `.segment` boundaries first (`wrapAcrossSegments`) —
that left the inter-segment gap's space text node unwrapped, producing a visible gap
in the middle of a cross-sentence highlight. Walking all text nodes directly instead
of segment-by-segment avoids that by construction, and also incidentally fixes the
inline-formatting multi-text-node case that the old per-segment `wrapInSegment` used
to silently skip.

**Not yet done — last-used color:** remember last picked color and pre-select it;
Enter confirms without needing to click. (Hook point is in the popover init,
`initAnnotationPicker` in `annotations.ts`.)

## UX — deleting an annotation (Stage 1) ✓ done

Right-click an annotated span → context menu with Delete option.

**Not yet done:** margin listen button alongside delete (mentioned as a Stage 2
possibility; Stage 2 shipped click-to-listen on the annotation mark itself
instead — see Known limitations under Stage 2).

## Implementation plan

### MVP ✓ done

1. **Rename** Edit view "Preview" → "Read" in `edit.ts` and `style.css`
2. **Create `annotations.ts`**:
   - `Annotation` type and `AnnotationColor` type
   - `loadAnnotations(docId)` / `saveAnnotations(docId, annotations)` via localStorage
   - `applyAnnotations(container, docId)` — walks DOM text nodes, matches, wraps in
     `<mark class="annotation" data-id="..." data-color="...">`
   - `wrapSelection(range, color, docId)` — creates annotation, saves, applies to DOM
   - `generateId()` — `crypto.randomUUID()` → strip hyphens → btoa
3. **Wire into `reader-author.ts`**:
   - Call `applyAnnotations(articleEl, docId)` after markdown render
   - On `mouseup` in `#article-area`: check selection, show color picker popover,
     call `wrapSelection` on color pick

### Stage 1 ✓ done

- Add `annotations.json` sidecar file per document (server-side)
- `GET /documents/:id/annotations` and `PUT /documents/:id/annotations`
  (body: `{ annotations, voice }` — voice triggers background alignment on save)
- Replace localStorage calls in `annotations.ts` with API calls
- Context-menu delete: right-click `.annotation` span → call delete API

### Stage 2 ✓ done (Kokoro and F5)

**Click an annotation mark → plays from sentence start, stops at word end.**

- `POST /voices/:voice_id/words` — body `{ sentence: string }`, returns `{ words: [...] }`
  - Kokoro: runs forced alignment directly on the raw sentence (lazy, cached
    in audio cache sidecar `.json`)
  - F5: normalizes the sentence first (same as synthesis does, so the cache
    key matches), runs alignment against the normalized text, then maps the
    aligned words back to original-text substrings — see Stage 3
- `tts/src/alignment.rs` — `ensure_words(key)`: reads cached words or runs
  forced alignment via `../forced-alignment` crate, writes result back to
  sidecar; per-key locked (mirrors `engine.rs`'s `synth_locks` pattern) so
  concurrent requests for the same sentence don't both redo the expensive
  decode+align step
- `app/frontend/src/player.ts`:
  - `segmentIndexForEl(el)` — maps segment DOM element → index
  - `listenTo(segIndex, endOffsetSecs)` — seeks to segment start, stops at offset
  - `stopAt` — checked each RAF tick; pauses when playback position reaches it
- `app/frontend/src/annotations.ts`:
  - `listenAnnotation(mark, annText, player, getVoice)` — POSTs to words endpoint,
    matches annotation text in word list, calls `player.listenTo`
  - `findAnnotationWordRange` stop-time: rather than a flat buffer after the
    matched word's end, stops just before the *next* word's onset (small
    safety margin) — a flat buffer either bled into the next word's audio or
    cut the current word short, depending on how tight the gap was, since
    forced alignment's *end* boundary for short words tends to land early
    relative to the more reliable *start* boundary of the following word
  - Loading state: dotted border in darker shade of annotation color (drawn inside
    highlight via `outline-offset: -2px`), cursor `wait`
  - Error state: red dotted border + `console.error` (debugging aid)
  - Generation counter (`listenGen`) discards stale fetch results if user clicks again
- `app/frontend/src/edit.ts`:
  - Delegated click handler on `articleContent` (capture phase) for `.annotation` marks
  - Capture phase prevents the segment's `seekTo` from also firing on the same click

**Word boundary snapping:** `wrapSelection` expands the selection left/right to
full word boundaries before saving and wrapping in the DOM. Prevents highlights
that clip the first or last letter of a word.

**Re-matching after edits:** `applyAnnotationToDOM` only requires the
40-char before/after `context` to match when the annotation's `text` is
ambiguous (appears more than once in the document). If `text` is unique,
it matches on text alone. This matters because `context`'s 40-chars-after
window can extend past the annotated phrase into the *next* sentence —
without this, editing that neighboring sentence (even unrelated to the
annotation itself) would change the recorded context and silently drop an
otherwise-untouched annotation.

**Per-word listen start offset ✓ done:** `listenTo(segIndex, endOffsetSecs,
startOffsetSecs)` in `player.ts` now accepts a start offset — `_doSeek`
trims that many seconds (converted to samples at 24000Hz) off the front of
the seeked-to segment's own audio before enqueuing it, leaving later
segments untouched. `findAnnotationWordRange` returns both `start` and
`end`: `start` is a small pre-roll before the first matched word's onset
(mirroring the end-boundary logic — clamped so it never overlaps the
previous word's audio), so listening starts right at the annotated phrase
instead of the whole sentence.

**Stop-at enforcement ✓ done (fixed a real bug):** `listenTo`'s stop point
was originally enforced only by `Player`'s `tick()` loop polling
`requestAnimationFrame` for `pos >= stopAt` and calling `pause()`.
`requestAnimationFrame` can be throttled by the browser when the tab loses
focus/visibility — confirmed via a cross-sentence annotation that logged
the *identical* computed `stopAt` on two different listens but produced
very different audible overshoots (once two full sentences plus two more
words, once just one extra word) — same input, different result, pointing
at enforcement timing rather than the offset math. Fixed by also
scheduling the cutoff natively: `AudioQueue` now tracks every
`AudioBufferSourceNode` it creates and exposes `scheduleStopAt(ctxTime)`,
which calls `.stop(ctxTime)` on all of them — sample-accurate, enforced by
the audio hardware clock, not the JS main thread. `listenTo` computes the
equivalent `AudioContext`-relative time (inverting the `position` getter's
formula: `ctxStopTime = firstStartTime + (stopAt - seekOffset)`) and calls
it right after `_doSeek`. `tick()`'s polling loop is kept for the
surrounding state sync (highlighting, calling `pause()` to update
`queue.state`) — if *that* lags under throttling, the audio is already
silent by then regardless, so only the cosmetic UI sync is delayed, not
the audible bug.

That first attempt still wasn't enough on its own — observed working
"usually on longer annotations" but not shorter ones. Cause: `_doSeek`
synchronously enqueues only the segments that already exist in
`this.segments` *at that moment*; synthesis keeps streaming new segments
in live via a separate path (the WS segment handler calls
`queue.enqueue()` directly), so a segment that arrives *after*
`scheduleStopAt()` was called but *before* the stop point isn't covered by
it — it just plays through uninterrupted. Longer annotations are more
often tested once synthesis has caught up (nothing left to stream in
mid-listen), so the gap rarely showed up there. Fixed by having
`AudioQueue` remember the active stop time (`scheduledStopAt`) and apply
it to every node *as it's created* in `enqueue()`, not just the ones that
existed when `scheduleStopAt()` was called. Cleared on `reset()` (any
plain seek goes through `_doSeek` → `queue.reset()`, so a stale cutoff
never leaks into unrelated playback) and when `tick()` actually reaches
`stopAt` (so segments arriving *after* the listen session ends don't
inherit it either).

**Known limitations / deferred:**
- Margin listen button not yet added
- Annotations still drop if the annotated text *itself* is edited (including
  case-only changes) — by design for now, since text is the anchor; no
  fuzzy/case-insensitive re-match yet

### Stage 3 ✓ done — F5 alignment via normalizer span mapping

Unblocked F5's `POST /voices/:voice_id/words` (previously 501) using the
span-mapping built into `util/src/normalizer.rs` — see
[normalize.md](normalize.md)'s Implementation section for the full
`Spanned`/`is_raw`/`source_range` design and why it's a forward mapping
rather than a post-hoc diff. Also a prerequisite for original-text playback
highlighting on any backend that normalizes text before synthesis (a second
TTS backend, CLI-only and not yet integrated into Odoru, will need this too).

**What was built:**
- `NormalizedText::source_range(normalized_range) -> Option<Range<usize>>` —
  the core lookup, maps a char range in normalized text back to the
  corresponding range in the original input
- `tts::alignment::words_with_original_text(words, normalized, original)` —
  for each aligned word (whose text reflects normalized-text form), finds
  its char range in the normalized text via sequential forward content
  search (robust to forced alignment dropping unalignable words — no need
  to track which positions were filtered), maps that range back to the
  original text via `source_range`, and substitutes the original substring
  for the word's `.word` field. Multiple aligned words landing in *overlapping*
  expanded source spans (e.g. all six of "Item seven one two seven nine"
  mapping to "Item 71279") are merged into a single output entry rather
  than returned as repeated duplicates. Merging is by range *overlap*, not
  exact equality — a word straddling a chunk boundary (e.g. "Winchester,"
  spanning the raw "Winchester" chunk plus the start of an expanded
  ", Massachusetts" chunk) computes a union range that's a strict superset
  of a neighboring word's pure single-chunk range; exact-equality merging
  missed this and produced a stray duplicate entry (e.g. a spurious ", MA"
  alongside "Winchester, MA") — caught from a real `/words` response, not
  just theoretically, fixed, and covered by a regression test using that
  exact sentence.
- `app/src/main.rs`'s `get_words` F5 branch: normalizes the client's raw
  sentence (`tts::f5::normalizer::normalize_with_spans`, the same call
  synthesis makes, so the cache key matches), runs `ensure_words`, then
  `words_with_original_text` before returning — the response shape is
  identical to Kokoro's, so `listenAnnotation` needed no client changes

**Known follow-up:** a normalizer chunk-granularity limitation (numbers
nested inside another pass's expansion, e.g. `<Ref-3>` → `Ref 3`, don't get
individually spelled out) could in principle cause a `<Ref-N>`-spanning
annotation to fail to align — see
[normalize-future.md](normalize-future.md)'s "Chunk-granularity limitation"
section for the detail. Not yet hit in practice for `<Ref-N>` specifically.

(A closely related instance of this *was* hit in practice for year ranges —
an annotation spanning `1973-76` found no word match at all, since
`expand_year_ranges` left both digit runs bare for Pass 7 to spell, but
Pass 7 can never reach digits inside an already-expanded chunk. Fixed by
having `expand_year_ranges` spell its own digits — see
[normalize.md](normalize.md)'s "Pipeline ordering matters" section.)

# Known limitations (cross-backend)

### Kokoro doesn't normalize — both an alignment gap and a real pronunciation bug

Kokoro's alignment ground truth (`meta.text`, used by `ensure_words`) is the
**raw**, unnormalized sentence text — Kokoro's cache key and synthesis input
are both raw too, so there's normally no mismatch. But Kokoro's own G2P
(misaki) apparently pronounces bare digits as words anyway (e.g. "2" spoken
as "two"), while the literal ground-truth text still says "2". Forced
alignment can't align a token with no letters against spoken audio, so it
gets silently dropped — confirmed via a server log warning:

```
WARN tts::alignment: alignment for <key>: 1 word(s) dropped before
alignment (no alignable chars): [FilteredWord { word: "2", original_index: 3 }]
```

Practical effect: an annotation spanning a bare number in a Kokoro-voiced
sentence may fail to find a word match (same symptom as the F5 issue Stage
3 fixed, but Kokoro was never in scope for that fix since it doesn't
normalize). Confirmed directly: an annotation ending in `"1973-76"` failed
to match because that token is completely absent from Kokoro's returned
words (filtered before alignment, not merely mismatched).

This isn't only an alignment problem — confirmed separately that Kokoro
also **mispronounces** "Winchester, MA" as literally "Winchester, ma"
(reading the postal abbreviation as a word) rather than "Winchester,
Massachusetts". `util::normalizer`'s state-abbreviation/number rules exist
precisely to fix this kind of thing for F5; Kokoro just never runs them.

Three options, not yet done:

1. **Ground-truth-only fix.** Run the same bare-number normalization on a
   copy of the text used only as Kokoro's alignment ground truth — not the
   cache key, not the synthesis input, both of which stay raw — then map
   aligned words back via `tts::alignment::words_with_original_text`, the
   same flow F5 already uses. Fixes the alignment gap only; does nothing
   for the mispronunciation, since Kokoro still synthesizes from raw text.
   **Risk:** assumes Kokoro's G2P reads digits the same way
   `util::normalizer`'s rules do — if Kokoro's actual convention differs,
   ground truth still won't match audio, just a different mismatch.
2. **Leave as a documented limitation** for now.
3. **Normalize for Kokoro too, same as F5** (preferred direction, not yet
   scoped/implemented) — run `util::normalizer::normalize` on Kokoro's
   synthesis input as well, not just the alignment ground truth. Fixes
   both problems at once: real pronunciation (the actual motivating case —
   "Winchester, MA" → "Winchester, Massachusetts" spoken correctly) and
   alignment (normalized text has no bare digits/abbreviations for forced
   alignment to drop). Bigger change than option 1: Kokoro's cache key
   would also need to switch from raw to normalized text (mirroring F5's
   design exactly), orphaning all existing Kokoro cache entries — expected
   per the project's existing cache-invalidation-via-key-change convention,
   not a special migration. `app/src/main.rs`'s `get_words` Kokoro/F5
   branches would also collapse into one shared code path. Still needs a
   listening-test pass across a broader sample before committing, since
   normalizer rules tuned for F5/VibeVoice's hallucination patterns may
   not all be necessary or even desirable for Kokoro's G2P.

# TODO

Not yet done — collected from the "Known limitations / deferred" and "Not
yet done" notes above:

Top priority:
- [x] **Per-word listen start offset** — `listenTo` now takes a start
  offset; `_doSeek` trims it off the front of the seeked-to segment's
  samples. See "Per-word listen start offset" under Stage 2 above.
- [x] **Cross-sentence annotations: create + match + render** — see
  "Cross-sentence selections" under UX — creating an annotation above.
- [x] **Cross-sentence annotations: click-to-listen** — `listenAnnotation`
  gathers all `<mark>` fragments sharing the clicked one's `data-id`
  (`wrapRange` gives each fragment the same id) to find the first/last
  touched segment. Single-segment annotations (still the common case) take
  the same one-fetch path as before. For a true cross-sentence annotation,
  fetches `/words` for the first and last segment in parallel, and matches
  each fragment's own text (not the full multi-sentence `annText`) against
  its segment's words — `findAnnotationWordRange`'s start from the first
  fetch, end from the second. `Player.listenTo` gained an `endSegIndex`
  param (defaults to `segIndex`) so `stopAt` can land in a later segment
  than playback started in. Also fixed: `deleteAnnotation` now unwraps
  *all* fragments sharing an id (`querySelectorAll`, was `querySelector`),
  and `.annotation`'s CSS rounds only the outer corners of the first/last
  fragment (`annotation-frag-start`/`-end`) so a multi-fragment highlight —
  including the lone-space gap between sentences — renders as one
  continuous bar instead of separate bubbles. The loading/error dotted
  indicator had the same problem one level deeper: it used `outline`,
  which can't be styled per-side, so every fragment drew its own full
  dotted box, doubling up at each seam. Switched to a real `border`
  (`box-sizing: border-box` so it doesn't shift surrounding text) with
  top/bottom always shown and left/right only on `annotation-frag-start`/
  `-end`, so the dotted line traces one open box around the whole
  annotation instead of one per fragment. Two more real bugs surfaced and
  fixed while testing this against live audio — see "Stop-at enforcement"
  under Stage 2 above: (1) the `tick()`/`requestAnimationFrame` polling
  loop that enforced `stopAt` could be throttled by the browser, letting
  audio audibly run past the cutoff by a variable amount; fixed by
  scheduling the stop natively on the Web Audio nodes
  (`AudioQueue.scheduleStopAt`). (2) that native schedule only covered
  nodes that existed at the moment it was called — a segment streaming in
  *after* via the live WS path bypassed it; fixed by having `AudioQueue`
  remember the active stop time and apply it to every node as it's
  created, not just the ones enqueued so far.
- [ ] **Margin listen button** — alongside the existing right-click delete;
  Stage 2 shipped click-to-listen on the annotation mark itself instead.

Not yet prioritized:

- **Last-used color** — remember the last picked annotation color and
  pre-select it in the popover; Enter confirms without clicking. Hook point
  is `initAnnotationPicker` in `annotations.ts`.
- **Fuzzy/case-insensitive re-match** — annotations still drop if the
  annotated text itself is edited, including case-only changes, since text
  is the literal anchor. Confirmed acceptable for now; revisit if it causes
  enough accidental annotation loss.
- **`<Ref-N>`-style chunk-granularity alignment gap** — see "Known
  follow-up" above and [normalize-future.md](normalize-future.md). Not yet
  hit in practice; low priority unless it actually breaks an annotation.
- **Ambiguous cross-sentence fragment match** — `listenAnnotation`'s
  cross-sentence path matches each fragment's own (possibly short) text
  against its segment's words via plain `indexOf`, with no context
  disambiguation. If that fragment's exact words happen to also appear
  earlier in the same sentence, the wrong (earlier) occurrence could be
  matched. Not yet hit in practice; the single-segment path already has
  proper context-based disambiguation (`applyAnnotationToDOM`'s ambiguity
  check) that this path doesn't reuse.
- **Diagnostic `console.log` in `listenAnnotation`'s cross-sentence path**
  — added while chasing the stop-at bugs above; per project convention,
  leave it until cross-sentence listening is confirmed solid across more
  varied test content, then remove.
