# TTS Artifact Suppression

## Problem

VibeVoice TTS occasionally hallucinates non-speech content into generated audio:

- Background music (often triggered by introductory phrases like "let me introduce")
- Impact/environmental sounds (door closing, muffled thuds)
- Crowd noise
- A different speaker (e.g. sounds like a voice in a different language)

Artifacts tend to appear at the **leading edge** of a chunk — beginning of a
paragraph, change of topic, or introductory sentence. At least one mid-segment
artifact has also been observed (seg 04, background noise mid-sentence).
Trailing-edge artifacts seem to happen less, perhaps since short sentences are
now merged with nearby paragraphs.

Audio is mono, 16-bit PCM, 24000 Hz (confirmed from sample output).

## Approach

### Pipeline order

Artifact suppression runs **after** AlignReport skip detection:

1. **Synthesize segment**
2. **AlignReport** — detect word skips via forced alignment; if a skip is
   found, re-render the segment (discard audio, loop back to step 1)
3. **Artifact suppression** — only runs on segments that pass the skip check
4. **Deliver**

This ordering avoids wasting suppression work on audio that will be discarded
anyway. It also means suppression never processes incomplete audio.

**Logging note:** if a skip and an artifact co-occur in the same segment before
re-render, log both together — co-occurrence patterns may reveal whether certain
hallucinations correlate with skips.

### Suppression scan window

Most artifacts are at the leading edge, but mid-segment occurrences have been
observed. The scan window should cover the full chunk, not just the leading
seconds — a full-chunk VAD pass is still cheap relative to synthesis time.

### Suppression steps

**First, evaluate DeepFilterNet** — a neural noise suppression model with a
native Rust implementation (`deep-filter` crate, ONNX or tract backend).
It handles music, impact sounds, and crowd noise in a single pass with no
pipeline to tune. It won't catch the wrong-speaker artifact (it treats all
speech as valid), but may solve the majority of cases with much less
complexity. Evaluate on the known artifact samples before building the
two-stage approach below.

Note: DeepFilterNet runs natively at 48kHz; audio is 24kHz — resampling
required, or check whether a 24kHz model variant is available.

If DeepFilterNet is insufficient, fall back to the two-stage approach:

1. **VAD scan** — detect non-speech regions across the full chunk; silence
   frames where no speech is detected
2. **Speaker verification** — embed windows against the reference profile for
   the expected voice; silence regions where the speaker doesn't match
3. **Pass cleaned audio downstream**

## Voice Reference Profiles

Speaker verification requires a reference embedding per voice:

- Build from a few minutes of known-good audio for each voice
- For the current production voice: extract from existing clean output
- For future voices: build into the synthesis UI (synthesize a short reference
  passage on first use, store the embedding)
- Profiles can be refined over time as more clean audio accumulates

## Latency

- Full-chunk VAD is still fast — much cheaper than synthesis itself
- Speaker embedding over sliding windows adds more cost but is still CPU-fast
- Goal: artifact check adds <500ms per chunk, well within acceptable latency
  for a paragraph-level pipeline (AlignReport already adds latency before this
  step)

## Fallback

If suppression degrades voice quality or introduces latency, the UI can be
updated to indicate the document is processing and present it fully assembled
rather than streaming sentence by sentence.

## Open Questions

- Does VAD trim alone catch most cases (music, door, crowd), or is speaker
  verification needed from the start?
- What is the right trim window — 1 second, 2 seconds, or adaptive?
- Where does suppression run — embedded in the synthesis service alongside
  AlignReport, or as a separate post-processing step?
- Does VAD alone catch most cases, or is speaker verification needed from the
  start?
