# Documents: markdown + plain-text, kept in sync

Every document keeps two text representations side by side:

- **`content`** — the markdown, what's shown/edited in the author UI.
- **`plain_text`** — derived from `content`, what sentence-splitting and TTS
  actually consume (annotations, the player, `tts::splitter::split`,
  `markdown.ts`'s mirrored client-side splitting).

They're stored together (`document.md` + the `plain_text` field) and always
updated as a pair — every write path that changes one recomputes the other.
Nothing re-derives `plain_text` from `content` lazily; whoever produces the
pair is responsible for both being correct at write time.

## The plain-text format

This is a contract specific to the `plain_text` field — not a property of
markdown, and not a property of plain text in general. It only holds because
all three producers below uphold it on the way out:

- **In markdown (`content`), a single `\n` within a paragraph is a soft
  wrap, not a break** — CommonMark renders it as a space; you need a blank
  line to start a new paragraph. **In arbitrary plain text** (e.g. some
  hard-wrapped `.txt` file that never went through one of the three
  producers below), a single `\n` could just as easily be a line-wrap inside
  one logical paragraph — there's no general rule that says otherwise.
- **In Odoru's `plain_text` specifically, neither of those ambiguities
  exists.** Every producer collapses any in-paragraph wrapping to a space
  *before* emitting, so by the time text reaches `plain_text`, a single `\n`
  is unambiguously a paragraph boundary — never a soft wrap inside one.
- **Blank lines are an optional, equivalent separator on top of that, not a
  requirement.** Real-world output varies — some producers emit a blank line
  between paragraphs, others emit none at all (single `\n`, no double `\n`
  anywhere). Both are valid `plain_text`; what's never valid in `plain_text`
  is a `\n` *within* one logical paragraph.

`util::splitter::split` is written against this `plain_text`-specific
contract — feeding it raw, un-normalized plain text (hard-wrapped or
otherwise) is out of contract and will over-fragment paragraphs.

`util::splitter::split` (the one place this format gets consumed) treats
every non-blank line as its own paragraph for exactly this reason — see its
doc comment for the full rationale. `markdown.ts`'s client-side sentence
splitting mirrors the same rule, so the two sides agree on sentence/paragraph
boundaries without needing to exchange any structure beyond the plain
strings themselves. `engine.rs`'s live synthesis loop uses the resulting
`paragraph_end` flag to choose a longer pause between paragraphs vs. a
shorter one between sentences in the same paragraph — get this wrong and
pauses land in the wrong places, or (worse) per-sentence indices drift
between client and server, corrupting highlighting, seeking, and audio-cache
lookups partway through a document.

## The three places that produce it

**1. Rust server — `tts::markdown::to_plain_text`** (`tts/src/markdown.rs`)

Used by the CLI for `.md`/`.html` file imports (`cli/src/main.rs`). Explicit
about the contract: joins blocks with `\n\n` and converts any in-block
soft/hard break to a space before emitting.

**2. Python extraction — `trafilatura.extract(..., output_format="txt")`**
(`dl/src/parser.py`)

Used for URL-fetched articles. Called as a second, independent extraction
pass alongside the markdown extraction (`output_format="markdown"`) — same
HTML in, two different `trafilatura` output formats out. Not under our
control in the same way as the other two, but empirically produces the same
one-line-per-paragraph shape.

**3. TypeScript client — `stripMarkdown`** (`app/frontend/src/edit.ts`)

Used when saving the editor's Text tab (`triggerSave`). Takes a different
route than the other two — renders markdown to HTML via `marked.parse`, then
strips tags with a regex — but lands on the same shape: `marked` runs in
default CommonMark mode (no `breaks: true` anywhere), so a soft line-wrap
inside one paragraph in the *source* markdown never survives as a literal
`\n` in the rendered HTML, and each block-level element ends up on its own
line once tags are stripped.

These three implementations share no code and were written independently —
the format is a convention they all happen to converge on, not something
enforced by a shared function. **Any new path that produces or edits
`plain_text` needs to uphold this shape by hand.** There's no validation
that catches a violation; it just silently desyncs client/server sentence
indices for that document until someone notices something is mis-highlighted
or mis-paced.

## vibe-imported documents are a fourth case, trivially

Documents created from a `dl import vibe` run don't go through any of the
three producers above — `document.txt` (the original source text vibe
synthesized from) *is* the `plain_text`, used as-is. It happens to already
be one-paragraph-per-line, blank-line-separated (confirmed directly against
`authorship` and `hypertext87`'s `document.txt`), so it satisfies the same
contract without any transformation step. `vibe/src/segment.rs`'s sidecar
builder splits sentences from this same text via `util::splitter::split`
(see `dev/tts-backends/vibe-import.md`) — relying on the identical format
contract described here, just arriving at the document by a different route.
