# Transclusion (Soon)

Odoru will support transclusion: a passage from document B embedded in document A by reference, not copy. When the player reaches a transclusion, it plays the audio from B in B's voice. The source connection is preserved — the reader always knows where the words came from.

Inspired by Ted Nelson's Xanadu concept, but adapted for an audio-first reading system.

## Markdown syntax

### Block transclusion

Used for standalone quoted passages (blockquotes).

```markdown
<!-- transclude -->
> Consider a future device for individual use, which is a sort of
> mechanized private file and library...It is an enlarged intimate
> supplement to his memory.
[(1, 106-7)](bush-as-we-may-think.md "Bush, V., 'As We May Think.' The Atlantic Monthly, p. 101-108; July, 1945.")
```

- `<!-- transclude -->` immediately before a blockquote marks it as a transclusion (not a regular blockquote)
- The blockquote contains the verbatim quoted text
- The citation link immediately after the blockquote provides:
  - **link text** — page/section reference (e.g. `(1, 106-7)`)
  - **href** — path or identifier of the source document
  - **title** — full bibliographic citation string

### Inline transclusion

Used for quoted passages embedded mid-paragraph.

```markdown
He summarized the situation: ["the growing mountain of research...square-rigged ships"](bush-as-we-may-think.md "Bush, V., 'As We May Think.' The Atlantic Monthly, p. 101-108; July, 1945.")
```

- A markdown link whose **link text is a quoted string** (starts and ends with `"`) is an inline transclusion
- Same href + title conventions as block transclusion
- Regular links (unquoted link text) are unaffected

## How the parser finds the passage in B

The verbatim blockquote or quoted link text is the anchor. At paste time, the client resolves the quoted text to a sentence index range in B and records it in B's `refs.json` sidecar (see Transcluded References below). At playback time, the pre-resolved offsets are used directly — no re-matching needed.

- No `words=` attribute in the markdown — the text itself is the reference
- If B's text changes, the stored offsets may drift → "newer version" indicator (see Versioning below)

## Rendering (GitHub Pages compatibility)

Block transclusions render gracefully as ordinary markdown on any standard renderer:
- `<!-- transclude -->` is an invisible HTML comment
- The blockquote renders as a blockquote
- The citation link renders as a normal link

Inline transclusions render as normal hyperlinks with quoted link text — readable, if not visually distinguished without Odoru's custom renderer.

## Playback behavior

- When the player reaches a transcluded passage, it switches to B's voice for that passage
- No announcement is made — the voice change itself signals the source shift
- The document A prose surrounding the transclusion describes who is being quoted (e.g. "He summarized the situation:")

## UI affordances

- Transcluded passages in A are visually distinguished (border, background, or source label — TBD)
- The citation link is always visible and clickable: jumps to document B at the relevant passage
- A "return" affordance brings the reader back to the position in A
- In document B, margin annotations mark passages that have been transcluded elsewhere, linking back to the referencing document

## Transcluded References

Each document that has been transcluded has a `refs.json` sidecar in its document store directory (`~/.odoru/documents/{uuid}/refs.json`). This is machine-maintained — updated whenever a transclusion of that document is pasted into any other document.

Each entry records:
- Sentence index range in B (start/end, resolved at paste time)
- The verbatim quoted snippet
- The referring document's ID and title
- The full citation string from the transclusion link

Example:
```json
[
  {
    "citation": "Bush, V., 'As We May Think.' The Atlantic Monthly, p. 101-108; July, 1945.",
    "refs": [
      {
        "referrer_id": "engelbart-augmenting",
        "referrer_title": "Augmenting Human Intellect",
        "sentence_start": 47,
        "sentence_end": 52,
        "snippet": "Consider a future device for individual use...",
        "resolved_at": "2026-06-08"
      },
      {
        "referrer_id": "engelbart-augmenting",
        "referrer_title": "Augmenting Human Intellect",
        "sentence_start": 12,
        "sentence_end": 14,
        "snippet": "the growing mountain of research...",
        "resolved_at": "2026-06-08"
      }
    ]
  }
]
```

The top-level array is grouped by citation (B's own bibliographic identity). Within each group, `refs` is a flat array of individual transclusion events — `referrer_id` and `referrer_title` repeat per ref, but in practice most docs will have only one or two refs per citation. Grouping by referrer in the UI is straightforward by filtering on `referrer_id`.

The `resolved_at` date provides the basis for drift detection: if B's text later changes, the stored sentence range may no longer match.

## Versioning / drift detection (future)

When B is edited after a transclusion was created, the anchor text in A may no longer match B exactly. A drift indicator (date stamp or symbol) will appear on the transclusion. Clicking it navigates to B so the reader can see what changed.

This is deferred until document editing is implemented.

## Stages

**Stage 1 (current design):** Transcluded text appears verbatim in A's markdown. The blockquote content must exactly match a passage in B.

**Stage 2 (future):** Paraphrased transclusion — A's text summarizes or paraphrases B's passage, but playback still uses B's audio for the original words.

**Stage 3 (future):** Nested transclusions — a transcluded passage may itself contain transclusions.

## Authoring flow (planned)

1. In the reader, select text in document B
2. Cmd/Ctrl+C — copies as transclusion by default; no special menu needed
   - `text/plain`: the transclusion markdown (block or inline syntax, verbatim text + citation pre-populated)
   - `text/plain` also includes a plain text version so paste works anywhere outside Odoru
   - Optionally a custom MIME type (e.g. `application/x-odoru-transclusion`) with structured JSON for paste-within-Odoru
3. Paste into document A's markdown editor — inserts the transclusion markdown
4. Right-click in the editor → "Paste as plain text" to paste the quoted text without transclusion markup
