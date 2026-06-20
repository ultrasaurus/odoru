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

### F5 alignment — open problem

F5's audio cache key is built from *normalized* text (numbers/abbreviations
expanded via `tts::f5::normalizer::normalize`), but annotations store the
*original* sentence text. `POST /voices/:voice_id/words` currently returns 501
for F5 because forced alignment would run against the normalized string,
producing word timestamps that don't line up with character offsets in the
original text used for annotation matching.

Kokoro's `ensure_words` (`tts/src/alignment.rs`) works because Kokoro uses raw
sentence text as both the cache key and the alignment input — no mismatch. F5
has no such symmetry. Before implementing F5's `/words` endpoint, design a
mapping step that aligns normalized-text words back onto original-text spans
(e.g. a diff/alignment between the two strings) — not just calling
`forced_alignment::align` on normalized text and returning those offsets
directly.

## Key files

- `app/frontend/src/annotations.ts` — annotation logic: create, apply, delete, listen
- `app/frontend/src/edit.ts` — wires annotation picker and click-to-listen handler
- `app/frontend/src/player.ts` — `listenTo`, `segmentIndexForEl`, `stopAt`
- `app/frontend/src/style.css` — `.annotation` styles, loading/error states
- `util/src/documents.rs` — `read_annotations` / `write_annotations` sidecar helpers
- `tts/src/alignment.rs` — `ensure_words`: lazy forced alignment, cached in sidecar
- `tts/src/audio_cache.rs` — `Meta` struct (now public, includes `words` field),
  `meta_path`, `mp3_path`, `read_meta`, `write_meta` helpers
- `tts/src/lib.rs` — exports `pub mod alignment`
- `app/src/main.rs` — REST endpoints: annotations CRUD + `POST /voices/:id/words`,
  background `align_annotations_for_doc` task on annotation save
- `dev/annotation.md` — this file

# Plan

## UX — creating an annotation

1. User selects text in Read mode via click-drag (normal browser selection)
2. On `mouseup`, if selection is non-empty and within `#article-area`: show a small
   color-picker popover near the selection with 5 color swatches
3. User clicks a color (or presses Enter to accept last-used color) → annotation saved
   → popover closes → highlight applied in-place
4. Escape or click-away dismisses without saving

**Cross-sentence selections (MVP):** if the selection crosses a `.segment` boundary,
trim to the sentence containing the drag anchor (start of selection). Avoids the hard
range-splitting problem for the MVP. Full cross-sentence support is post-MVP.

**Last-used color:** remember last picked color and pre-select it; Enter confirms
without needing to click. (Detail to fill in; hook point is in the popover init.)

## UX — deleting an annotation (Stage 1)

Right-click an annotated span → context menu with Delete option.
Stage 2 may add a margin listen button alongside this.

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

### Stage 2 ✓ done (Kokoro; F5 returns 501)

**Click an annotation mark → plays from sentence start, stops at word end.**

- `POST /voices/:voice_id/words` — body `{ sentence: string }`, returns `{ words: [...] }`
  - Kokoro: runs forced alignment (lazy, cached in audio cache sidecar `.json`)
  - F5: returns 501 NOT_IMPLEMENTED (alignment requires normalizer merge; deferred)
- `tts/src/alignment.rs` — `ensure_words(key)`: reads cached words or runs
  forced alignment via `../forced-alignment` crate, writes result back to sidecar
- `app/frontend/src/player.ts`:
  - `segmentIndexForEl(el)` — maps segment DOM element → index
  - `listenTo(segIndex, endOffsetSecs)` — seeks to segment start, stops at offset
  - `stopAt` — checked each RAF tick; pauses when playback position reaches it
- `app/frontend/src/annotations.ts`:
  - `listenAnnotation(mark, annText, player, getVoice)` — POSTs to words endpoint,
    matches annotation text in word list, calls `player.listenTo`
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

**Known limitations / deferred:**
- F5 alignment not yet implemented (needs normalizer merge to map word timestamps
  back to original text — see Stage 3)
- Listen starts from sentence start (no per-word start offset yet)
- Cross-sentence annotations not yet supported
- Margin listen button not yet added

### Stage 3 (not started) — F5 alignment via normalizer span mapping

Unblocks F5's `POST /voices/:voice_id/words` (currently 501) and is also a
prerequisite for original-text playback highlighting on any backend that
normalizes text before synthesis (a second TTS backend, CLI-only and not yet
integrated into Odoru, will need this too).

**Problem:** F5's cache key and alignment input are *normalized* text
(numbers expanded, acronyms spelled out, etc. — see `util/src/normalizer.rs`).
Annotations store *original* sentence text. Running forced alignment on
normalized text produces word timestamps whose character offsets don't
correspond to positions in the original text.

**Design (from a separate design session):** the normalizer emits a forward
mapping — output span → source span — alongside its normalized string,
rather than reconstructing the mapping by diffing normalized vs. original
after the fact. Diffing is fragile because it silently breaks when
normalization rules change shape; a forward mapping stays correct by
construction.

**Plan:**
- Refactor `normalizer.rs` so each transform pass threads span information
  through (see scoping notes — separate from this doc, large enough to need
  its own design pass before implementation)
- `POST /voices/:voice_id/words` for F5: run alignment on normalized text,
  translate each word's normalized-text span back to original-text offsets
  using the mapping, return the same `{ words: [...] }` shape Kokoro already
  returns — client-side `listenAnnotation` code needs no changes
- Until this lands: F5 stays 501; playback highlighting (a related, broader
  feature) falls back to showing normalized text rather than original
