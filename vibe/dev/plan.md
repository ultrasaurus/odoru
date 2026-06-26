# Plan: Evaluate vibe voice

## Goal
- To see if we can generate 15-20 minutes of text that Sarah can listen to
  without triggering a migraine.  If so, integrate into CLI. Consider how to
  integrate into Odoru.

[*] - means in-progress, implementation done, needs testing

Steps
1. [x] Section A issues from [dev/normalize-future.md](../dev/normalize-future.md):
   1. [x] Acronym spelling (A1): review existing test coverage for the
      3-letter-acronym rule; add cases for an acronym that should be
      pronounced as a word, and confirm the override map can force
      that behavior over the default spell-out.
   2. [x] Em dash (A2): `--` → `, ` confirmed working 2026-06-18 after
      segmented listen tests (see normalize-future.md section G).
      - TODO: verify em-dash fix still working in current test runs
   3. [x] Ref/code patterns (A3): bracket-stripping + punctuated-override
      fixes, confirmed passing in normalize-future.md sections E/F.
2. [x] Listen test: create audio wav files for sections of
   `odoru/data/authorship.txt` (workspace-root source doc, distinct from
   `vibe/data/` segment output):
    * [x] Markers
    * [x] Traveling Through the Working Files (`augment_traveling`)
    * [x] Supporting Multi-Party Collaboration (`augment_multiparty`)
3. [x] Write new test sentences that form a reasonably realistic prose
   paragraph covering Section B in
   [dev/normalize-future.md](../dev/normalize-future.md), create
   audio, listen test.
4. [X] `augment_multiparty` (and likely other multi-paragraph files)
   speed up noticeably toward the end at cfg=2.0 — before tackling
   full-file stitching, chunk `augment_multiparty.txt` into its
   individual paragraphs, generate audio per paragraph, and listen to
   see whether each paragraph alone stays at normal speed (i.e.
   confirm the speed-up is a function of cumulative generation length,
   not the content itself).
5. [x] Create audio file for all of `odoru/data/authorship.txt` by dividing
   into text segments at paragraph division then stitching audio together.
   - [x] Split into segments via segmenter (50–200 words, split at paragraph
     boundaries) after resolving seg07 clipping issue
   - [x] Seed discovery: ran seg07–11 with 5 different seeds; seed 71463
     chosen as preferred voice (see `vibe/dev/voices.md`)
   - [x] Full run: seg01–seg26 with seed 71463
   - [x] Stitch: seg12–16, seg16–25 stitched; seg26 (References) separate
   - [x] **Resolved**: repeated-phrase hallucination on seg10 was VRAM-related
     (RTX A4000 16GB). Re-run on RTX A6000 48GB — clean. Fixed by enforcing
     >=24GB VRAM minimum in `new-pod`.
6. [x] Update docs and investigate error from last run
   - [x] update docs
   - [x] seg33 — root cause was RunPod proxy 524 timeout on long segments
     (blocking `/synthesize` held connection open during inference).
     Fixed by replacing blocking `/synthesize` with async job API:
     `POST /jobs` returns immediately, CLI polls `GET /jobs/:id`,
     fetches wav via `GET /jobs/:id/wav`. seg33 (284 words, RTF 1.04x
     on RTX 3090) completed successfully on 2026-06-19. Docker image
     bumped to v13.
7. [x] Augment fixes: from full document test, see [artifact-augment.md](artifact-augment.md)
   - [x] update `tts_overrides.txt`
   - [x] NAME,number, pattern → normalizer fix
   - [x] segmentation: reduced max segment size from 250 → 200 words to
     prevent truncation and speed degradation
   - [*] QA pass with forced-alignment AlignReport 
     (detection of word-skipping/truncations)
8. [*] Don't store reference voices in Dockerfile
   - [x] --voice option
   - [x] test with Sarah
   - [x] test with Andy
9. [x] eval Google Cloud Run
   - [x] Cloud Run project setup, Dockerfile.cloudrun
10. Improve tooling to improve workflow
   - [x] log GPU type and VRAM of selected pod at run time (currently only
     filtered by >=24GB, not recorded); useful for correlating artifact
     patterns with hardware 
   - [x] segmentation (integrated in Rust CLI) - max word limit enforced at 200
   - [x] recently vibe-service failed -- long segment timout (polling fixes)
   - [x] JobState for resilience
   - [x] rerun - test augment segments to validate skipping
   - [x] Move to Blackwell GPU - Dockerfile.cloudrun-blackwell
11. Validation
   - [X] Full doc run of Hypertext87
     - [ ] listen test
     - [ ] test CLI import with audio
   - [ ] implement playback of partial synth => test with unfinished Augment 
12. Improve Workflow part 2
   - [ ] CUDA alignment (now the real bottleneck at batch scale —
     see `dev/cloudrun/cloudrun-blackwell.md` N=49 alignment finding)
   - [ ] continue batch testing 64+
13. Consider additional improvements
   - background noise removal
   - use forced-alignment to find original headers and add silence

## Known TTS truncation cases

Segments where the model truncated audio before the end of the text. Goal is to
find general normalizer fixes rather than document-specific patches.

| Segment | Words | Suspected cause |
|---------|-------|-----------------|
| seg20   | 247   | Paragraph 2 contains quoted CLI commands with `!!` and uneven quote stripping — model may bail early on malformed quoted text |
