# Silent Text

Some text in a document is there to *read* but not to *hear*: section
headings inserted after the fact for navigation, editorial labels, bracketed
insertions that were never part of the original narrative. Silent text is
displayed in the body and the outline, but excluded from speech synthesis and
playback highlighting.

Status: implemented (headings + standalone block paragraphs; inline silent
deferred).

## Motivation

The driving case is `hypertext87` — a transcript of a talk whose `##` headings
("Doug Engelbart", "Ted Nelson", ...) were added later as navigation aids.
They were never spoken. Read aloud verbatim, the synthesizer announces each
heading as if it were a sentence, which is wrong. We want the headings on
screen and in the outline, but silent.

## Marker

A silent span is bracketed text immediately followed by an HTML comment:

```markdown
[Doug Engelbart]<!--silent-->
```

For the inserted-heading case:

```markdown
## [Doug Engelbart]<!--silent-->
```

Why this shape:

- **Brackets** are the standard editorial convention for text that was
  inserted, not present in the original. They render as literal
  `[Doug Engelbart]` in any CommonMark renderer (an undefined shortcut
  reference link is *not* linkified), so the text reads correctly on GitHub
  with no live link.
- **`<!--silent-->`** is the explicit machine signal. HTML comments are
  invisible in every markdown renderer, so the marker adds no visible noise
  while remaining unambiguous to parse. Brackets alone would be implicit and
  would collide with any other literal bracket use.

Rejected alternatives: `[text](silent:)` becomes a live link in standard
parsers (and GitHub's sanitizer strips both the href *and* the brackets);
`{silent}` shows literally on GitHub and gives no bracket display; bare
`[text]` is clean but implicit/ambiguous. See the design discussion for the
full comparison.

### Scope (first pass)

- Silent **headings** and **standalone block paragraphs**.
- Mid-sentence inline silent runs are deferred — they would split a sentence
  span, are fiddlier, and there is no current need.

## Rule

> Render the bracketed text in the body **and** the outline; exclude it from
> TTS and from playback highlighting.

## Why it needs almost no plumbing

The server only ever synthesizes from `document.txt` (plain text); it never
sees the markdown. So:

> **silent = present in `.md` (content), absent from `.txt` (plain_text)**

The server needs **no change**. The two requirements are:

1. The plain-text derivation (`tts::markdown::to_plain_text`) must drop silent
   spans, so they never reach the synthesizer.
2. The client renderer must render silent text **without** creating a
   synthesis span or advancing the global sentence index — so client highlight
   indices stay aligned with the server's, which they do automatically because
   `.txt` omits the same text.

## Implementation

### Plain-text derivation — `tts/src/markdown.rs`

`to_plain_text` is the canonical exclusion point: it is what `dl edit
--format markdown` runs to derive `plain_text` from edited markdown
(`cli/src/main.rs`). Add a silent pre-pass that removes
`\[…\]<!--\s*silent\s*-->` and drops any heading line left empty by the
removal. Unit-tested alongside the existing inline-stripping tests.

### Renderer — `app/frontend/src/markdown.ts`

`renderToken` is shared by the authoring reader, the export SPA, and the
edit-mode preview, so one change covers all three paths. (The two
`SentenceProvider` implementations it delegates to for non-silent blocks —
the wasm-backed one in `markdown-live.ts` for the live app, the precomputed
one in `markdown.ts` for the export — are not shared, but neither needs to
know about silent handling; see "Shared code boundary" in `dev/export.md`.)

In `renderToken`, detect the marker on `heading` and `paragraph` tokens:

- Render the element with the brackets kept (comment stripped) and a `silent`
  CSS class.
- Silent **headings** are still pushed to the outline, with
  `sentenceIndex = globalIdx` — which now points at the *next real* sentence,
  the natural scroll target.
- Do **not** call `provider.weave()`, do **not** push to `pendingSpans`, do
  **not** advance `globalIdx`.

### Authoring-edit derivation — `app/frontend/src/edit.ts`

`stripMarkdown` derives `plain_text` for in-app edits. Apply the same silent
pre-pass so app edits produce the same `plain_text` the Rust path would —
keeping the TS and Rust rules mirrored (as they already are for inline
stripping).

### Styling — `app/frontend/src/style.css`

Style `.silent` (muted / italic, with a small "not read aloud" affordance).

## Authoring workflow (hypertext87)

Edit **only** `data/hypertext87.md` — wrap the inserted headings as
`## [Heading]<!--silent-->`. There is no need to hand-edit `.txt`; the
`--format markdown` path derives it with silent text stripped.

```bash
# hypertext87 already exists in the store; use its doc id.
dl edit 4fccf37a-19c5-4dae-8084-9a0c56702bc4 data/hypertext87.md --format markdown
```

`update_content` swaps content + `plain_text` while preserving the document's
UUID and `source_url`, and marks synthesized voices stale. So the re-import
keeps the document associated with its original source URL; a re-synth picks
up the new (shorter) sentence list.

### Note on sentence indices

This applies only when *retrofitting* silent markers onto a document that was
**already synthesized** (the hypertext87 case). Removing heading lines from the
spoken text shifts every subsequent sentence index, which invalidates the
existing audio (voices go stale, re-synth needed). A document authored with
silent markers from the start never goes stale this way — it is synthesized
once against text that already excludes the silent spans. Either way, the
shift does **not** desync highlighting: both the server and the client
recompute indices from the same text, so they stay consistent.
