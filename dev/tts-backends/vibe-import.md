# Design: `dl import` — CLI side of vibe import prep

Companion to `vibe/dev/odoru-import-prep.md`, which designs the sidecar format
vibe writes. This doc covers the Odoru CLI command that consumes it.

Status: design discussion, not yet implemented. Implementation starts
once this doc is approved.

## Decisions informing this design

- **New subcommand on the existing `dl` binary** (`cli/src/main.rs`),
  not a separate tool: `dl import <basedir>`.
- **One command with a future `--segment <n>` flag**, not a separate
  subcommand for single-segment re-import. Whole-document import ships
  first; `--segment` is a later addition.
- **Find a matching document by hashing existing docs' plain text,
  not by an explicit `--doc-id` flag.** The sidecar's `source_sha256`
  is `sha256_hex()` of the plain-text source file vibe split (see
  `vibe/src/segment.rs::sha256_hex`/`build_sidecar`) — the same
  algorithm `util::index::html_content_hash` uses, just over plain
  text instead of HTML. Odoru's existing `content_hash` field on a
  fetched document is `sha256(source.html)`, a different hash space,
  so there's no existing index to look this up by. Instead: for each
  existing document, hash its `plain_text` and compare to
  `source_sha256`. Match → attach to that document, leaving its
  existing content/metadata untouched. No match → create a new
  document from the source `.txt` (`content` = `plain_text` = source
  text, `content_hash` = `source_sha256`).
  - This replaces an earlier draft decision ("always create a new
    document") — dropped once we noticed two real test documents
    originated from `dl fetch` and already exist in the store.
  - Linear scan over `documents::list_all()` + `lookup_by_id` per
    candidate. No new index needed — this is a one-off CLI command,
    not a hot path.
- **Per-segment failures are skip-and-report, not abort-on-first-error.**
  A document ends up partially populated rather than the whole import
  failing because one segment's transcript didn't parse. Easier to
  debug — you see exactly which segments succeeded.
- **Segment-level cache keys must be scoped per document**, not the
  plain `audio_cache::cache_key(text, voice)` used by deterministic
  backends (F5/Kokoro). Reusing the plain key would let an imported
  sentence collide with — or be silently reused by — another document
  with identical sentence text, which is unsafe here because vibe's
  output isn't deterministic per text+voice (see "Caching assumption
  that doesn't hold for imported audio" in
  `vibe/dev/odoru-import-prep.md`).
  - The document id is embedded in `voice_id` itself —
    `"vibevoice:default:<doc_id>"` — rather than threaded through the
    cache-key call as a separate argument, because the read side
    (`/voices/:voice_id/words`, `handle_synth`) only ever receives
    `voice_id` and raw sentence text. There's nowhere else in the
    existing request shape to carry a document id through to the cache
    lookup. See "Playback" below for why this matters and how it's
    wired through.
  - The key includes a running **sentence index** (document-wide, not
    reset per segment): `audio_cache::cache_key(sentence_text,
    &format!("{voice_id}:{sentence_index}"))`, with `voice_id` already
    carrying `doc_id` (see below) — note `voice_id` here, not a derived
    `Voice::cache_key()` value, since an imported voice has no `Voice`
    struct/registry entry at all (see "Playback").
  - This was initially dropped as unworkable, since neither
    `WordsRequest` (`{ sentence: String }`) nor `handle_synth`'s
    per-sentence loop carries an occurrence index today, so the read
    side had no way to reconstruct it. Restored once it was clear
    `vibe-playback.md`'s player redesign already needs every imported
    sentence's real index threaded through end to end (to place
    out-of-order arrivals and gaps correctly in its now
    index-addressable `segments` array) — the read-side protocol is
    being extended for vibevoice playback anyway, so passing the index
    through is no extra surface, not a new one. See "Playback" for the
    request-shape change this implies.
  - Without it, two identical sentences in the same imported document
    would collide and the second's audio would silently win for both —
    the same category of risk the sidecar design doc flags for vibe's
    non-determinism generally, just self-inflicted within one document
    instead of across documents. The index closes that gap entirely.
- **Partial import does not mark the voice `Ready`.** The player allows
  seeking into a not-fully-rendered voice and playing whatever's
  already there, so there's no need to invent a new `VoiceStatus`
  variant for "partially imported" — reuse the existing states
  faithfully:
  - All sentences across all segments imported → `Ready`.
  - At least one sentence imported, but not all → `InProgress`.
  - Zero sentences imported → `Error`.
- **`hound` is a new dependency for the CLI crate** to decode
  `_generated.wav` into samples before slicing/re-encoding per
  sentence. Already used elsewhere in this workspace (`vibe/Cargo.toml`),
  so no new tooling, just a new dependency edge.
- **Vibe's wav output is mono** — no downmixing needed before handing
  samples to `audio_cache::encode_mp3`.
- **Each `_transcript.json` covers exactly one segment.** Vibe
  synthesizes and aligns strictly per-segment, so the importer can
  flatten all `Word`s across a transcript's `Segment`s in order without
  needing to reconcile multiple unrelated segments in one file.

## Why normalization is needed before mapping words back

Forced-alignment `Word` timestamps are positions in whatever text was
actually fed to the TTS/aligner — the *normalized* text (numbers
expanded, `Speaker 1: ` prefix stripped, etc.) — not the sidecar's
`sentences[].text`, which is an exact, non-normalized substring of the
original document (e.g. `"71279"`, not `"seven one two seven nine"`).

Without translating transcript word positions back to original-text
substrings, a sentence's text and the transcript's words wouldn't line
up at all, breaking the content-search match needed to find a
sentence's time range in the segment audio.

`util::normalizer::normalize_with_spans(original_segment_text)` keeps a
source-span mapping; `tts::alignment::words_with_original_text(words,
normalized, original)` uses it to rewrite each `Word`'s text back to
its original-text substring (merging multiple normalized words that
map to one original span, e.g. all six of "Item"/"seven"/"one"/
"two"/"seven"/"nine" → `"Item 71279"`), keeping the same
segment-relative timestamps. This is exactly the mapping already built
and proven for F5 (see "Normalized-to-original word mapping" in
`vibe/dev/odoru-import-prep.md`) — reused here, not re-derived.

## Playback — why it can't be deferred, and how it has to work

Checked `tts/src/backend.rs`'s `Voice`/`Backend` enums and the full
request path in `app/src/main.rs`. The gap is bigger than "playback
doesn't know the key yet" — the server has no routing path for a
"vibevoice" voice at all today, and the one it does have actively
forces live synthesis on a cache miss, which must never happen for
imported audio.

**The gap, concretely:**

- `AppState` (`app/src/main.rs:123-138`) holds exactly two engines:
  `kokoro: Option<Arc<TtsEngine>>` and `f5: Option<Arc<TtsEngine>>`,
  each built once at server startup from a fixed `Backend` config with
  a static voice registry (`tts/src/engine.rs` `TtsEngineBuilder::build`).
  There is no third slot, and no mechanism to register a voice at
  runtime — every voice a `TtsEngine` knows about was decided at
  process start.
- `AppState::engine_for_voice` (`app/src/main.rs:149-163`) matches
  literally on `"f5"` / `"kokoro"` and errors `"unknown backend"` for
  anything else. A `voice_id` like `"vibevoice:default"` fails here
  today.
- Even with a third arm added, `TtsEngine::synthesize`
  (`tts/src/engine.rs:99-120`, `run_synthesis_loop`) calls
  `backend.synthesize_sentence(...)` on a genuine cache miss — i.e. the
  existing engine abstraction *is* "check cache, then run a real model
  if it's not there." There is no live vibevoice model in the Odoru
  server process (vibe runs synthesis on a separate RunPod pod, offline,
  ahead of time) — so this path must never be reached for an imported
  voice. A cache miss for imported audio has to be a reported gap, not
  a trigger to synthesize.
- `get_words` (`app/src/main.rs:304-351`) builds its cache key from
  `engine.voice_cache_key(voice_name)` — i.e. it's coupled to the same
  per-process `Voice` registry, not just to `engine_for_voice`'s
  backend dispatch.

**Resolution:**

- `voice_id` for an imported document is `"vibevoice:default:<doc_id>"`
  (general form: `"<backend>:<voice>:<doc_id>"`), written into
  `voices.json` by the importer and used by the client unchanged
  everywhere it already sends `voice_id` today (`/voices/:voice_id/words`
  path param, `SynthRequest.voice`). No new request fields needed —
  `parse_voice_id`'s `split_once(':')`
  (`app/src/main.rs:175-177`) already only splits on the *first* colon,
  so `"vibevoice:default:<doc_id>"` parses as
  `("vibevoice", "default:<doc_id>")` without changes to that function.
- Both `get_words` and `handle_synth` need a new branch, checked before
  `engine_for_voice`: if the backend prefix is `"vibevoice"` (or
  whatever set of imported backends exist), skip `engine_for_voice` and
  the `TtsEngine` entirely:
  - `get_words`: `WordsRequest` needs a new field — a sentence index —
    for imported voices (`{ sentence: String, sentence_index: Option<usize> }`,
    or split into a separate request variant; exact shape is an open
    item below). Build the key as `audio_cache::cache_key(&body.sentence,
    &format!("{voice_id}:{sentence_index}"))` and call
    `tts::alignment::ensure_words(&key)` as today. This already works
    unmodified once the key is right — `ensure_words`
    (`tts/src/alignment.rs:34-58`) only ever reads `Meta.words` from the
    cache or re-derives them by decoding the cached mp3 and re-running
    forced alignment; it never touches a `TtsBackend`. Since the
    importer writes `Meta.words` directly at import time, this hits the
    fast path immediately.
  - `handle_synth`: needs a genuinely new code path — a "replay-only"
    version of `run_synthesis_loop` that splits `req.text` into
    sentences (same `splitter::split` call, which already produces a
    real index per sentence — `tts/src/engine.rs:140`'s
    `sentences.into_iter().enumerate()`), and for each one does
    `audio_cache::lookup(cache_key(sentence_text, &format!("{voice_id}:{index}")))`,
    streaming a segment on a hit and a gap/error marker — carrying that
    same real index — on a miss. Never calls `engine.synthesize()` or
    any `TtsBackend` method. The gap marker carrying the real index is
    what `vibe-playback.md`'s index-addressable `segments` array needs
    to place a miss correctly and still learn the document's total
    sentence count even with gaps.
- This is consistent with what you flagged as the good news: once the
  key format is right, the *shape* of the per-sentence request/response
  protocol (one `/words` call per sentence, one segment per sentence in
  the synth stream) doesn't need to change much — the index is a small
  addition to existing per-sentence calls, not a redesign. Only the
  routing (`engine_for_voice`-equivalent dispatch) and the
  synthesis-vs-replay behavior on a miss are genuinely new.
- Still open: how the frontend should render a missing/un-imported
  sentence during playback of a partially-imported (`InProgress`)
  document — whether existing handling for "synthesis still catching
  up" on normal `InProgress` docs already covers this, or needs its own
  treatment. Worth checking against `dev/frontend.md` and the player
  code before implementation, not resolved in this doc yet.

## Command flow (draft)

`dl import <basedir>`:

1. **Locate the sidecar.** Find the single `*.segments.json` file in
   `<basedir>`. Error if zero or more than one is found — same
   "operator always names the one they mean" philosophy as
   `--basedir` itself; no auto-disambiguation.
2. **Find or create the target document**, per the hash-matching
   decision above.
3. **For each segment, in order** (skip-and-report on failure):
   - Read `files.transcript` → `forced_alignment::transcript::Transcript`;
     flatten `words` across its `segments`.
   - Read `files.original`; run `normalize_with_spans`.
   - `words_with_original_text(words, normalized, original)` → words
     with original-text substrings + segment-relative timestamps.
   - Decode `files.audio` (mono wav) via `hound` → f32 samples + sample
     rate.
   - For each `Sentence` in this segment's `sentences`: find its
     contiguous run of words by content search (same technique
     `words_with_original_text` already uses internally), take
     `first_word.start .. last_word.end`, slice the PCM samples,
     `encode_mp3`, and `audio_cache::store` under the scoped cache key
     above.
   - Any failure (missing file, parse error, no word match for a
     sentence) → log a warning naming the segment/sentence, skip just
     that unit, continue.
4. **Write `voices.json`** for the target document: `voice_id` from
   the sidecar, status per the partial-import rule above.
5. **Print a summary**: document id, segments imported/skipped,
   sentences imported/skipped, with reasons for any skips.

## Open items

- Exact `clap` arg shape for `dl import` (just `<basedir>`? a `--title`
  override for the no-match-found create-new path?).
- Whether `dl import` needs a `--dry-run` to preview the
  match/create decision and segment-by-segment plan without writing
  anything.
- Exact `WordsRequest`/`SynthRequest` shape change to carry a sentence
  index for vibevoice voices — a new optional field on the existing
  structs, or a separate request variant gated on the voice_id prefix.
  Affects the frontend call sites too (`player.ts`'s words-fetch and
  synth-request code), not just the server.
