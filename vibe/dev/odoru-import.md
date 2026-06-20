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

(1) is a deeper problem deferred for later — see "Open: normalized to
original word mapping" below. This doc focuses on (2) and (3), which
are solvable now.

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

### Normalized-to-original word mapping
The forced-alignment transcript's words are positions in *normalized*
text, not the original. Normalization (`util/src/normalizer.rs`) does
many-to-many text transforms (number expansion, roman numerals,
acronym spelling, etc.) with no span tracking today. Two options,
not yet decided:

- (a) Reverse-map after the fact (diff-based reconstruction) — fragile,
  since normalization rules change over time.
- (b) Have the normalizer emit a forward mapping (output span ->
  source span) as it transforms text, alongside its output string.

(b) is preferred since it doesn't need to be re-derived every time
normalization rules change, but isn't designed yet. Needed before
Odoru can highlight the *original* document text in sync with
playback of normalized-text-driven audio; until then, playback
highlighting can fall back to showing normalized text.
