# Design: importing vibe output into Odoru

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
- Today there's no manifest tying these back to character offsets in
  the original document.
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

## What's missing today

1. No mapping from a segment's normalized text back to original-text
   word positions. The forced-alignment transcript (`_transcript.json`)
   only has word timestamps for the *normalized* text.
2. No mapping from a segment to its character offsets in the original
   source document.
3. No record of paragraph boundaries within a segment.
4. No record of which voice/speaker config actually produced a
   segment's audio.

(1) already has a solution to reuse, not design from scratch — see
"Normalized-to-original word mapping" below. (2), (3), and (4) are
what the sidecar format in this doc addresses.

## Design: emit offsets at split time

Rather than reconstructing segment boundaries after the fact (e.g. via
substring search against the original document), vibe should compute
and record them when it splits the document, since it already knows
exactly where each cut falls.

### Move `splitter::split` into `util`

`Sentence` and `splitter::split` currently live in `tts/src/splitter.rs`.
Vibe already depends on `util` (not `tts`), so move this module to
`util/src/splitter.rs`. This also means Odoru's own TTS pipeline and
vibe's segmenter share one paragraph-boundary rule instead of vibe
re-deriving its own.

`Sentence` needs byte-offset fields added (it currently only carries
`text` and `paragraph_end`):

```rust
pub struct Sentence {
    pub text: String,
    pub start_offset: usize,   // UTF-8 byte offset into source text
    pub end_offset: usize,
    pub paragraph_end: bool,
}
```

Note: offsets are only meaningful relative to whichever text was
passed into the `split()` call that produced them — `Sentence` doesn't
carry any reference back to *which* source text or document it came
from. In principle the same `Sentence` value could be (mis)matched
against a different text than the one it was derived from; in practice
this basically never happens (each call site immediately consumes its
own output), so we're not adding a source-text identity field for it.

### Vibe's split step

At split time, vibe runs `splitter::split` once over the *whole*
source document, then groups the resulting sentences into its existing
segment boundaries (today: never mid-sentence, and currently never
mid-paragraph either — see "Open: paragraph splits within a segment"
below for why that may change). Each segment retains the
`start_offset`/`end_offset`/`paragraph_end` of its component
sentences.

### Sidecar format: `<docname>.segments.json`

One file per document, written alongside the existing `_segNN.*`
files:

```json
{
  "source_document": "authorship.txt",
  "source_sha256": "3a7f...e91c",
  "voice_id": "vibevoice:default",
  "segments": [
    {
      "index": 1,
      "start_offset": 0,
      "end_offset": 842,
      "sentences": [
        { "start_offset": 0,   "end_offset": 118, "paragraph_end": false },
        { "start_offset": 119, "end_offset": 301, "paragraph_end": true  }
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

- `start_offset`/`end_offset` are UTF-8 byte offsets into
  `source_document`, matching what Rust string slicing and
  `unicode_segmentation` already use.
- `source_sha256` is a hash of the original document at split time.
  If the source is edited later, Odoru's importer can detect the
  mismatch and refuse/warn rather than import against stale offsets.
- `segments[].start_offset`/`end_offset` are kept even though they're
  currently derivable from the first/last sentence in `sentences`.
  This is deliberate redundancy: once segmentation needs to split
  *within* a paragraph (long monologues — Ulysses' Molly Bloom
  soliloquy is ~3,687 words in one sentence-ish span — or simply very
  long single sentences), a segment's edges may no longer coincide
  with a sentence boundary. Keeping the segment's own span authoritative
  avoids relying on `sentences` always bracketing it exactly, and gives
  O(1) "which segment contains offset X" lookups without scanning.
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

### Caching assumption that doesn't hold for imported audio

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
   is being given) before trusting offsets.
2. For each segment: transcode `_generated.wav` to mp3 into Odoru's
   existing audio cache (`tts/src/audio_cache.rs`), keyed the normal
   way; copy `_transcript.json` words into the cache `Meta.words`
   field.
3. Write/attach the sidecar (or a derived form of it) so Odoru's
   reader can map playback position back to source-document offsets
   and render paragraph breaks correctly, and so a later "re-run this
   segment" action knows exactly which span of the original document
   to re-normalize and re-synthesize.

## Open questions / deferred work

### Paragraph splits within a segment
Vibe's segmenter currently never splits mid-paragraph. If/when long
single paragraphs (or even single sentences) need splitting for
synthesis length limits, the segment-level `start_offset`/`end_offset`
become load-bearing rather than redundant — see note above. No design
needed yet beyond making sure the sidecar format already supports it,
which it does.

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
