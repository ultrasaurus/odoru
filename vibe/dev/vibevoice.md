# VibeVoice evaluation notes

Evaluating [vibevoice-community/VibeVoice](https://github.com/vibevoice-community/VibeVoice)
as a possible TTS backend for Odoru. See [plan.md](plan.md) for the
evaluation plan and [dev/normalize-future.md](../dev/normalize-future.md)
for normalizer issues found along the way.

## Listen test procedure

See [listen-test.md](listen-test.md) for the current procedure (uses the
`synthesize` command via `vibe-service`). The old SSH-based workflow is
preserved in [old-listen-test-ssh.md](old-listen-test-ssh.md) and will
be deleted once the new path is confirmed reliable.

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

## Run log

- 2026-06-15, pod `ypl1py60u8knen`: GPU NVIDIA RTX 4000 Ada (20GB) per
  `nvidia-smi`, 16 vCPU, 62GB RAM, $0.26/hr. The first attempt (04:55
  UTC) ran on CPU instead of GPU — a fresh venv's torch was reinstalled
  as a cu130 build, incompatible with this pod's cu124 driver, so
  `torch.cuda.is_available()` was False (~4.5s/it, projected ~70min).
  Once fixed to use system python (torch already cu124, GPU available),
  both files generated at GPU speed: `augment_multiparty` (98.67s
  audio) in 89.76s, RTF 0.91x; `augment_traveling` (66.67s audio) in
  59.27s, RTF 0.89x — both ~$0.26/hr, so a few cents each.

## Open questions / not yet evaluated

- Generation speed (wall-clock per minute of audio) on the GPU pod —
  not recorded.
- Multi-speaker support (VibeVoice supports multiple speakers; we've
  only used "Speaker 1" so far).
- Long-form stitching — cfg=2.0 per-chunk quality holds up (speed-up
  artifact resets per chunk, not cumulative). Voice consistency at
  segment boundaries: same seed (71463) makes transitions sound like
  "a real human shifting speech after a breath" — acceptable at
  paragraph breaks, but needs more listening to confirm. May also need
  a volume normalization post-processing pass to even out levels across
  segments.
