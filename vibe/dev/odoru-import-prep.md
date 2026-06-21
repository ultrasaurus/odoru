# Design: prep for importing vibe output into Odoru

## Goal
Preserve vibe's per-segment synthesis output (audio + word timestamps)
so it can be ingested into Odoru's document/audio cache, with enough
fidelity to re-run an individual segment later without re-deriving
boundaries from scratch.

## Background

Vibe splits a source document (e.g. `odoru/data/authorship.txt`) into
segments, normalizes each segment's text, synthesizes audio, and
produces a forced-alignment transcript per segment.

- Segment files: `authorship_seg01.txt` ... `segNN.txt`.
- Today there's no manifest tying these back to the original document
  or to each other beyond filename order.
- The only ordering signal is filename order (`_segNN`) and an ffmpeg
  concat list used for playback.

Odoru's own cache (`util/src/documents.rs`) tracks synthesis state per
*document*, not per segment.

- There's no existing notion of segment-level audio/status in Odoru's
  core model.
- We don't want to force one in: other TTS backends (sentence-chunked,
  sometimes reassembled for long sentences) don't need segment
  identity at all.
- Adding segment-awareness to Odoru's core schema would leak
  vibe-specific chunking assumptions into a model used by backends
  that don't share them.

Decision: segment metadata lives in a **sidecar file** that Odoru's
importer reads, not in Odoru's core document/voice-state schema. Odoru
reads the sidecar if present and ignores it otherwise.

### Basedir — which run directory is canonical

Independent of this feature: `vibe/data/` currently mixes loose
top-level `_segNN.txt` files with per-document directories
(`authorship/`) that themselves contain multiple run subdirectories
(`authorship-skips`, `authorship-full-doc-1`, `authorship segment
tests`), with no marker for which one is the canonical/current output
for that document.

Resolution: no naming convention or pointer file. Both vibe (when
writing the sidecar) and Odoru's importer (when reading it) take an
explicit `--basedir <path>` argument naming the directory to operate
on. There is no automatic "this is the current one" inference —
the operator always says which directory they mean. Experimental/test
run directories are simply never passed as a basedir to import.

## What's missing today

1. Vibe import doesn't yet *use* the existing normalized↔original word
   mapping. The forced-alignment transcript (`_transcript.json`) only
   has word timestamps for the *normalized* text — `util::normalizer`
   already solves mapping that back to original-text positions (built
   for F5), it just hasn't been wired into vibe import yet. See
   "Normalized-to-original word mapping" below — no new design needed,
   just reuse.
2. No record of paragraph boundaries within a segment.
3. No record of which voice/speaker config actually produced a
   segment's audio.

(2) and (3) are what the sidecar format in this doc addresses.

## Move `splitter::split` into `util`

`Sentence` and `splitter::split` currently live in `tts/src/splitter.rs`.
Vibe already depends on `util` (not `tts`), so move this module to
`util/src/splitter.rs`. This also means Odoru's own TTS pipeline and
vibe's segmenter share one paragraph-boundary rule instead of vibe
re-deriving its own.

`Sentence` doesn't need any change — it already carries exactly what's
needed (`text: String`, `paragraph_end: bool`).

## Vibe's split step

At split time, vibe runs `splitter::split` once over the *whole*
source document, then groups the resulting sentences into its existing
segment boundaries (today: never mid-sentence, and currently never
mid-paragraph either — see "Paragraph/sentence splits within a
segment" below for why that may change). Vibe writes the resulting
`Sentence` list (`text` + `paragraph_end`) for each segment straight
into the sidecar — the importer does not re-run `splitter::split`
itself.

Re-running the splitter at import time instead of storing its output
would risk drift: if `splitter::split`'s rules change between when
vibe rendered the audio and when import runs later, recomputation
could produce a different sentence/paragraph split than what the
audio was actually generated against, silently misaligning paragraph
breaks from what was actually spoken. Storing the actual sentence list
vibe used avoids that — same category of problem `source_sha256`
already guards against for the whole document, just at a finer grain.

## No offsets in the sidecar

Earlier drafts of this design added `start_offset`/`end_offset` to
`Sentence`, to each sidecar sentence entry, and to each segment, for
mapping playback position back into the full original document.
Walking through actual use cases shows none of them need it:

- **Building cache entries / slicing audio per sentence** (the
  importer's main job): needs each sentence's text and its matching
  word timestamps. Text is already in `Sentence.text` — no offset math
  needed to get it. Matching words to a sentence is a content-search
  problem (find which transcript words fall within this sentence's
  text), the same technique `words_with_original_text` already uses —
  not an offset-arithmetic problem.
- **Paragraph break rendering**: `paragraph_end: bool` per sentence is
  already sufficient — render order + that flag tells the reader to
  insert a break after this one. No position numbers required.
- **Re-running a single segment**: the segment's original text is
  already available verbatim in `_segNN.txt` — no need to re-slice it
  out of the full document by position.
- **Highlighting the full document during playback**: this is already
  solved a different way — by capturing surrounding context, not by
  storing character positions (positions are expensive to keep valid
  as the document is edited). Since segment text is an exact,
  non-normalized substring of the document, a position can always be
  cheaply re-derived later via substring search if some future feature
  genuinely needs one — no reason to store and maintain it now.

So the sidecar carries no offsets at all, at any level.

## Sidecar format: `<docname>.segments.json`

One file per document, written alongside the existing `_segNN.*`
files:

```json
{
  "schema_version": "0.1",
  "source_document": "authorship.txt",
  "source_sha256": "3a7f...e91c",
  "voice_id": "vibevoice:default",
  "segments": [
    {
      "index": 1,
      "sentences": [
        { "text": "Augment's mail system allows one to send complete, structured documents as well as small messages.", "paragraph_end": false },
        { "text": "In an authorship environment, an important role for electronic mail is for the control and distribution of documents, where small, throw away messages are considered to be but a special class of document.", "paragraph_end": true }
      ],
      "files": {
        "original":   "authorship_seg01.txt",
        "normalized": "authorship_seg01_normalized.txt",
        "audio":      "authorship_seg01_generated.wav",
        "transcript": "authorship_seg01_transcript.json",
        "report":     "authorship_seg01_report.json"
      }
    }
  ]
}
```

Notes on the fields:

- `schema_version` starts at `"0.1"` — this format isn't proven yet.
  Bump to `"1.0"` once import has been implemented end-to-end and a
  document imported through it has actually been listened to in
  Odoru.
- `source_sha256` is a hash of the original document at split time.
  If the source is edited later, Odoru's importer can detect the
  mismatch and refuse/warn rather than import against a document that
  no longer matches what was synthesized.
- `sentences` are exactly the `Sentence` values vibe's split step
  produced for this segment — `text` + `paragraph_end`, no offsets
  (see "No offsets in the sidecar" above).
- `index` matches the existing `_segNN` filename suffix, so the
  manifest stays correlated with files even if a segment is
  individually re-run/replaced later.
- `files` is a relative-path lookup table rather than an assumed
  naming convention, so import doesn't break if vibe's file naming
  changes.
- `voice_id` is vibe's own identifier for whichever
  speaker/model/config actually produced this audio (e.g.
  `"vibevoice:default"`). Vibe knows this already — it shouldn't be
  re-supplied at import time. The audio is already rendered by the
  time import runs, so asking the operator for a `--voice` flag would
  be redundant at best and a source of mislabeling at worst (operator
  error could tag vibevoice-rendered audio as `"f5:sarah"` in Odoru's
  catalog). Import reads `voice_id` from the sidecar and writes it
  into `voices.json` (`util/src/documents.rs`) as-is, or through a
  fixed mapping table if vibe's naming and Odoru's catalog naming
  ever diverge — but the source of truth for *which voice rendered
  this* is always the sidecar, not an import-time argument.
- `files.report` (`_report.json` — filtered/suspect word counts from
  forced alignment) is intentionally not consumed by import. It's kept
  for manual QA only; no design need yet to surface it programmatically.

## Caching assumption that doesn't hold for imported audio

Odoru's audio cache (`tts/src/audio_cache.rs`) is keyed by
`sha256(normalized_text + voice)` — it assumes identical text from the
same voice always produces identical-sounding audio, so it's safe to
dedupe/reuse by content hash. That assumption holds for Odoru's
existing deterministic backends (Kokoro, F5), but not for vibe (output
varies run to run even for the same text/voice) or a future
human-recorded import (the same sentence read aloud twice will sound
different each time).

The sidecar avoids relying on that assumption: each segment's audio
file is tied to a specific *occurrence* (`files.audio` in the
manifest), not derived by re-hashing the segment's text. Import should
not attempt to fold vibe-imported audio into the text-hash-keyed cache
as if it were interchangeable with other audio for the same text.

## How Odoru's import would use this (future step)

Not building yet, but the import command's job, given the sidecar:

1. Verify `source_sha256` matches the document Odoru already has (or
   is being given) before trusting the rest of the sidecar.
2. For each segment: transcode `_generated.wav` to mp3 into Odoru's
   existing audio cache (`tts/src/audio_cache.rs`); copy
   `_transcript.json` words (mapped back to original text — see
   "Normalized-to-original word mapping" below) into the cache
   `Meta.words` field.
3. Write `voice_id` into `voices.json` (`util/src/documents.rs`).
4. Use `sentences`' `paragraph_end` flags to render paragraph breaks
   correctly when displaying the imported document, and to know
   exactly which span of text to re-normalize and re-synthesize if a
   segment is later re-run.

## Re-running a single segment after import

Not a CLI/API workflow yet — just manual steps for now:

1. Identify the problem segment.
2. Re-run it manually via vibe, regenerating that segment's
   `_generated.wav`/`_transcript.json`/`_report.json` in place (the
   sidecar's `sentences` only need updating if the segment's *text*
   changed, not just its audio — most re-runs won't touch the sidecar
   at all).
3. `odoru import` gets a new option to import a single segment by
   index — re-transcodes just that segment's audio, re-derives its
   original-text word mapping from the refreshed transcript, and
   overwrites only that segment's entries in the audio cache, leaving
   every other segment untouched.
4. Restart the Odoru server — its cache/document state is loaded at
   startup, so a cache file overwritten on disk isn't picked up by a
   running server. This is a known limitation, not something this
   design solves.

## Open questions / deferred work

### Paragraph/sentence splits within a segment
Vibe's segmenter currently never splits mid-paragraph or mid-sentence.
If/when long single paragraphs (or even single sentences — long
monologues, e.g. Ulysses' Molly Bloom soliloquy at ~3,687 words in one
sentence-ish span) need splitting for synthesis length limits, this
will need its own design pass. The sidecar format above doesn't yet
account for a segment boundary falling inside a sentence.

### Multi-speaker text
Sample segment text has a `Speaker 1: ` prefix per line. Not addressed
here: whether that prefix is stripped before going into `Sentence.text`
/normalization, and how a future multi-speaker document with different
voices per speaker would map onto this design's single document-level
`voice_id`. Left open.

### Normalized-to-original word mapping — solved for F5, reusable here
This was an open problem when this doc was first drafted; it's since
been implemented for F5 and the same pieces apply directly to vibe
import:

- `util::normalizer::normalize_with_spans(text) -> NormalizedText`
  runs the same passes as `normalize()`, but threads a source-span
  through every pass instead of producing a flat string. `.text` is
  the same string `normalize()` would return.
- `NormalizedText::source_range(normalized_range) -> Option<Range>`
  maps a char range in the normalized output back to the char range
  in the original input it came from. Mapping is per-chunk, not
  per-char — if a range spans multiple chunks, the result is the union
  of their source ranges.
- `tts::alignment::words_with_original_text(words, normalized, original)`
  takes forced-alignment `Word`s (positions in normalized text), the
  `NormalizedText`, and the original text, and returns `Word`s with
  `.word` rewritten back to the original-text substring (e.g.
  "seven"/"one"/"two"/"seven"/"nine" merge back to "71279"). Words
  with no alignable source span are skipped without throwing off
  subsequent matches, since matching is sequential content search, not
  position arithmetic.

For vibe import this means: don't re-derive normalized↔original
mapping independently. Run each segment's original text through
`normalize_with_spans` (the same normalization vibe already applies to
produce `_normalized.txt`), then feed the segment's
`_transcript.json` words through `words_with_original_text` to get
word timestamps keyed to *original* text. That's what lets Odoru
highlight the original document during playback instead of showing
normalized text — no separate design needed here.
