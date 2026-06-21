# Future Design Notes

## Multiple Auhors (hosted server environment)

### Authentication

- evaluate UI surface for APIs that need additonal guards (e.g. delete, patch). 
- Likely need concept of document owner and admin role.
- shared cache? 

### Per-user annotations

Annotations (see [annotation.md](annotation.md)) currently live in a per-document
`annotations.json` sidecar, keyed by document UUID — single-user only. Once
multiple authors share a document store, annotations need to move to
user-scoped storage so each author's highlights are private. Document UUID
keying makes this migration straightforward when auth lands.

## Static export
- See [export.md](export.md) for current implementation & CLI usage, meets primary use case of demo deployed via github pages
- Export UI in authoring is expected to be needed when there are multiple users who want to post their projects as static web pages, preconditions:
  - decide if public fetched URLs are shared across users
  - separate document stores per user for orginal in-progress works
  - if public documents are shared, publish choices still need to be per user
- UI design
  - probably a button in the reader
  - Consider a warning for incomplete audio
- Each user needs their own artifact, so zip-download seems the right approach

## Scalability

The dedup indexes (`source_url.json`, `content_hash.json`) are simple JSON files, fine for a personal tool with ~100s of articles. If odoru ever needs to handle many concurrent users or large article counts, these would need to move to a proper database or at minimum a single-writer queue. Not a concern now but worth knowing the boundary.

## Audio disk cache: no eviction — grows unbounded
See [tts-backend/cache.md](tts-backend/cache.md) for cache details.
needs a cleanup strategy (mark-and-sweep; entries already support `invalid: bool` / `invalid_reason` fields for this)

**Idea:** a mark-and-sweep GC pass should scan `~/.odoru/audio/` for `invalid: true` entries
(and optionally entries older than a TTL) and delete the `.mp3` + `.json` pair. The `invalid_reason`
field leaves room for additional invalidation sources (`("manual"`, `"ttl"`).

## Sentence-level source offsets (`start_offset`/`end_offset` on `Sentence`)

Came up while scoping `vibe/dev/odoru-import.md`'s segment-sidecar design,
then dropped from that doc — the use case there didn't actually need it
(derivable from other data if needed later). But the underlying capability
— knowing each sentence's byte/char offset range in the original source
document — would benefit a few other things if ever built:

- **Original-text playback highlighting for normalizing backends.**
  `annotation.md`'s Stage 3 already notes this as a prerequisite but
  doesn't say how it'd wire up: `NormalizedText::source_range` maps a
  word's position within *one sentence's* normalized text back to that
  sentence's original text, but there's no link from "this sentence" to
  "where it sits in the full document." Sentence-level offsets would close
  that gap — compose sentence-offset + intra-sentence `source_range` to
  get a full document-level range, letting F5 (or any normalizing backend)
  highlight the right span of the original rendered document during
  playback, not just the original sentence text in isolation.
- **More robust annotation anchoring.** Today annotations anchor by
  literal text + ~40-char fuzzy context (see annotation.md's Anchoring
  section) — which is exactly what broke when an edit to a *neighboring*
  sentence changed the captured context for an untouched annotation (fixed
  for now by only requiring context when the text is ambiguous). Sentence
  offsets would let an annotation anchor to (sentence identity,
  intra-sentence offset) instead, immune to edits elsewhere by
  construction rather than by disambiguation heuristic.
- **Incremental re-synthesis on edit.** `edit.ts`'s `saveAndSynth` always
  re-synthesizes the whole document on any text change, even a one-word
  edit in one paragraph. Costly for F5 (~0.17x realtime). Sentence offsets
  would let a future diff step identify which sentences are unchanged (by
  offset + content) and skip re-synthesizing those.

None of these are designed — just noting that if any of them get picked
up, sentence-level offsets are the shared building block, so don't
re-derive the idea from scratch.
