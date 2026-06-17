# Annotations

Authors can select text (word-to-phrase granularity) and highlight it with a color,
like marking up a paper with highlighters. Independent of audio playback highlighting.

## Terminology

- **Annotation**: a colored highlight on a span of text, created by the author
- Distinct from the audio "highlight" (active sentence during playback)

## Scope

- Author Read view only (not the publish-preview / export SPA)
- Single-user for now; multi-user deferred (login coming later)

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

## Storage

- **MVP**: `localStorage["odoru:annotations:<docId>"]` — JSON array of `Annotation[]`
- **Stage 1**: Persist to server (sidecar file `annotations.json` alongside `voices.json`)
  - Keyed by document UUID; easy to migrate to per-user storage when auth lands
- **Stage 2+**: per-user annotations, served from user-scoped storage

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
- `GET /documents/:id/annotations` and `PATCH /documents/:id/annotations`
- Replace localStorage calls in `annotations.ts` with API calls
- Context-menu delete: right-click `.annotation` span → call delete API

### Stage 2

- Listen to highlighted text: play only the audio segments that overlap with annotated spans
- Margin button per annotation as alternate entry point for delete / listen

## Key files

- `app/frontend/src/annotations.ts` — new module (all annotation logic)
- `app/frontend/src/reader-author.ts` — wire in applyAnnotations + mouseup handler
- `app/frontend/src/edit.ts` — rename Preview → Read
- `app/frontend/src/style.css` — `.annotation` styles + color variants + popover
- `util/src/documents.rs` — (Stage 1) annotations sidecar file path
- `app/src/` — (Stage 1) REST endpoints for annotations
