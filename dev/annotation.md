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

**Cross-sentence selections (MVP):** if the selection crosses a `.segment` boundary,
trim to the sentence containing the drag anchor (start of selection). Avoids the hard
range-splitting problem for the MVP. Full cross-sentence support is post-MVP.

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

**Known limitations / deferred:**
- Listen starts from sentence start (no per-word start offset yet)
- Cross-sentence annotations not yet supported
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
  for the word's `.word` field. Multiple aligned words landing in the same
  expanded source span (e.g. all six of "Item seven one two seven nine"
  mapping to "Item 71279") are merged into a single output entry rather
  than returned as repeated duplicates.
- `app/src/main.rs`'s `get_words` F5 branch: normalizes the client's raw
  sentence (`tts::f5::normalizer::normalize_with_spans`, the same call
  synthesis makes, so the cache key matches), runs `ensure_words`, then
  `words_with_original_text` before returning — the response shape is
  identical to Kokoro's, so `listenAnnotation` needed no client changes

**Known follow-up:** numbers/IDs nested inside a different pass's expansion
(e.g. the `3` in `<Ref-3>` → `Ref 3`) don't get individually spelled out by
the bare-number rule, since chunk granularity is per-expansion, not
per-word — see normalize.md's "Granularity is per-chunk" note. Not yet
hit in practice for annotations, but worth knowing if a `<Ref-N>`-spanning
annotation ever fails to align.
