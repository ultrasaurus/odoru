# Transclusion (Soon)

Odoru will support transclusion: a passage from a source document embedded in
a referencing document by reference, not copy. When the player reaches a
transclusion, it plays the audio from the source document in the source
document's voice. The source connection is preserved — the reader always
knows where the words came from.

Inspired by Doug Engelbarts Augment system and Ted Nelson's Xanadu concept, but 
with a different UX and adapted for an audio-first reading system.

Initial implementation will support transclusion within a single website by a
single author or team, implying that authors can write to both documents
which live inside a single Odoru instance. (See "Scalability" section
below for future plans.)

* [Markdown syntax](markdown.md)
* [Inbound references](inbound-references.md)
* [Word-level playback](word-level-playback.md)


## How the parser finds the passage in the source document

The verbatim blockquote or quoted link text is the anchor. At paste time, the
client resolves the quoted text to a word offset range in the source document
and records it in the source document's `refs.json` sidecar (see
[Inbound references](inbound-references.md)). At playback time, the
pre-resolved offsets are used directly — no re-matching needed.

- No special attribute in the markdown — the text itself is the reference
- If the source document's text changes, the stored offsets may drift →
  "newer version" indicator (see Versioning below)

## Rendering (GitHub Pages compatibility)

Block transclusions render gracefully as ordinary markdown on any standard renderer:
- `<!-- transclude -->` is an invisible HTML comment
- The blockquote renders as a blockquote
- The citation link renders as a normal link

Inline transclusions render as normal hyperlinks with quoted link text — readable, if not visually distinguished without Odoru's custom renderer.

## UX
### Listening experience

- When the player reaches a transcluded passage, it switches to the source document's voice
- No announcement is made (for stage 1) — the voice change itself may be 
  sufficent signal of the source shift
- Typically, a document will contain prose surrounding the transclusion describes who is being quoted (e.g. "He summarized the situation:")

### UI affordances

- Transcluded passages in the referencing document are visually distinguished
  (border, background, or source label — TBD)
- The citation link is always visible and clickable: jumps to the source
  document at the relevant passage
- A "return" affordance brings the reader back to the position in the
  referencing document
- In the source document, margin annotations mark passages that have been
  transcluded elsewhere, linking back to the referencing document


## Authoring flow (planned)

1. In the reader, select text in the source document
2. Cmd/Ctrl+C — copies as transclusion by default; no special menu needed
   - `text/plain`: the transclusion markdown (block or inline syntax, verbatim
     text + citation pre-populated)
   - `text/plain` also includes a plain text version so paste works anywhere
     outside Odoru
   - Optionally a custom MIME type (e.g. `application/x-odoru-transclusion`)
     with structured JSON for paste-within-Odoru
3. Paste into the referencing document's markdown editor — inserts the
   transclusion markdown
4. Right-click in the editor → "Paste as plain text" to paste the quoted text
   without transclusion markup

# Open Questions (defered)
* Voice shift UX: after first implementation, consider extra silence or adding
  the word "quote" which is often how reader voice that shift, yet any of these
  could be as unsettling as the voice shift
* With the specific text as the anchor, it could be repeated elsewhere in the
  document (likely to be rare in practice)

# Future 
* **Paraphrased transclusion**: the referencing document summarizes or
  paraphrases a passage, but playback still uses the source document's audio
  for the original words (in the context of the source document).
* **Nested transclusions** a transcluded passage may itself contain transclusions.
* **Exteral transclusions** transcluded links between two Odoru sites

## Versioning / drift detection
When the source document is edited after a transclusion was created, the anchor
text in the referencing document may no longer match. A drift indicator (date
stamp or symbol) will appear on the transclusion. Clicking it navigates to the
source document so the reader can see what changed.

This depends on general version/change tracking feature implementation.

## Scalability
Later external inbound references could notify a site,
like 2002-era TrackBacks by Moveable Type. A user could choose to publish
external reference or not (or something in-between like an allow-list,
including sites I link to can reference my site). Outbound references would
only be bi-directional if the other-side is also an Odoru-hosted site and
accepts the link. The published site could still be CDN-hosted static content
with a small dynamic component that controls rules-based (or interactive)
updates to reference descriptors.



# End Notes
[1]: https://dougengelbart.org/content/view/148/#7d3
[2]: https://dl.acm.org/doi/10.1145/800197.806036