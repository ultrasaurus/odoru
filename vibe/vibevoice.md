# VibeVoice evaluation notes

Evaluating [vibevoice-community/VibeVoice](https://github.com/vibevoice-community/VibeVoice)
as a possible TTS backend for Odoru. See [plan.md](plan.md) for the
evaluation plan and [dev/normalize-future.md](../dev/normalize-future.md)
for normalizer issues found along the way.

## Test inputs (`data/*.txt`)

All derived from `data/authorship.txt` (the augment/NLS "Authorship
Provisions" paper) via `tts/examples/normalize_dump.rs`
(`cargo run --example normalize_dump < input.txt > output.txt`):

- `odoru_test_normalized.txt` — full `authorship.txt`, normalized.
  61 lines/paragraphs.
- `odoru_abstract_intro_normalized.txt` — first 10 lines only
  (Abstract + Introduction + first Authorship paragraph), normalized.
  Used as a quick smoke test before running the full file.
- `odoru_markers_normalized.txt` — just the "Markers" section (12
  lines), normalized. Used to compare normalizer output against the
  raw source (`data/markers.txt`) and find the issues now tracked in
  `dev/normalize-future.md`.

Corresponding `*_generated.wav` files are the VibeVoice output for
each — gitignored (large binary), live in `vibe/data/` locally.

## Qualitative notes (listening)

- Voice used: `voices/sarah/ref.wav` (custom reference voice, not one
  of the stock `vv/demo/voices/*`).
- CFG (classifier-free guidance) scale matters a lot:
  - At the default cfg 1.3 (used for the per-section test files —
    markers, abstract/intro): longer silences between sentences, and
    audible crowd-noise/background artifacts.
  - At cfg 2.0: silences are normal length, crowd noise mostly gone
    for short sections.
  - However, rendering the *whole* `odoru_test_normalized.txt` file
    at cfg 2.0 introduces new artifacts — barks, squeaks, and other
    glitches — with increasing frequency after ~6-7 minutes in. Also,
    narration speed gradually increases over the course of the file.
  - This degradation-over-length is the main motivation for the
    segmentation approach in plan.md step 4 (split into ~300-word
    chunks, generate separately, stitch together) — full-file
    generation in one pass doesn't hold up past ~6-7 minutes.
- Normalizer-driven mispronunciations found by listening to the
  Markers section — see `dev/normalize-future.md` Section A
  (acronym splitting, em-dash, ref/code patterns) for specifics.
- "Move" -> "moove" and alphanumeric ID spacing ("3b5" -> "3 b 5")
  sounded fine in this voice (Section B in normalize-future.md,
  still wants broader test coverage).

## Open questions / not yet evaluated

- Generation speed (wall-clock per minute of audio) on the GPU pod —
  not recorded.
- Multi-speaker support (VibeVoice supports multiple speakers; we've
  only used "Speaker 1" so far).
- Long-form stitching (plan.md step 4) — not started. Needs design
  for how cfg=2.0's per-chunk quality holds up once chunks are
  generated and joined 
  - does the speed-up/artifact pattern reset per
    chunk, or accumulate across the stitched output?
  - do the voice characteristics match enough or does it sound like there
    are jumps between different speakers? (consider using same seed?)
