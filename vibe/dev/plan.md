# Plan: Evaluate vibe voice

## Goal
- To see if we can generate 15-20 minutes of text that Sarah can listen to
  without triggering a migraine.  If so, integrate into CLI. Consider how to
  integrate into Odoru.

Steps
1. [x] Section A issues from [dev/normalize-future.md](../dev/normalize-future.md):
   1. [x] Acronym spelling (A1): review existing test coverage for the
      3-letter-acronym rule; add cases for an acronym that should be
      pronounced as a word, and confirm the override map can force
      that behavior over the default spell-out.
   2. [x] Em dash (A2): `--` → `, ` confirmed working 2026-06-18 after
      segmented listen tests (see normalize-future.md section G).
   3. [x] Ref/code patterns (A3): bracket-stripping + punctuated-override
      fixes, confirmed passing in normalize-future.md sections E/F.
2. [x] Listen test: create audio wav files for sections of
   `data/authorship.txt`:
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
5. [x] Create audio file for all of `data/authorship.txt` by dividing
   into text segments at paragraph division then stitching audio together.
   - [x] Split into segments via `split_authorship.py` (250–400 words),
     then resplit from seg12 onward at 150–250 words via
     `split_authorship_short.py` and `split_authorship_end.py` after
     seg07 clipping issue
   - [x] Seed discovery: ran seg07–11 with 5 different seeds; seed 71463
     chosen as preferred voice (see `vibe/dev/voices.md`)
   - [x] Full run: seg01–seg26 with seed 71463
   - [x] Stitch: seg12–16, seg16–25 stitched; seg26 (References) separate
   - [x] **Resolved**: repeated-phrase hallucination on seg10 was VRAM-related
     (RTX A4000 16GB). Re-run on RTX A6000 48GB — clean. Fixed by enforcing
     >=24GB VRAM minimum in `new-pod`.
6. Update docs and investigate error from last run
   - [ ] update docs
   - [ ] seg33 -- `vibe/data/authorship_seg33.txt` No log was downloaded — 
     the client got the 524 error before
     inference completed (or before it could fetch the log). The pod was
     terminated so we can't retrieve it now. The 524 means the RunPod proxy 
     closed the connection after its timeout, but inference may well have
     finished on the pod afterwards — 
     we just weren't there to receive it. Consistent with the pattern we saw 
     with a prior job (338 words): completes on the pod, client never gets the
     response. To confirm the theory, next time a 524 happens we could SSH in 
     immediately after and check /tmp/ for the wav.

7. Improve tooling to improve workflow
   - [ ] consider additional synthesize/vibe-service testing needed before
     removing listen-test-ssh (recently vibe-service failed -- need to
     investigate)
   - [ ] consider QA pass with forced-alignment AlignReport
8. Consider additional improvements
   - background noise removal
   - use forced-alignment to find original headers and add silence

## Known TTS truncation cases

Segments where the model truncated audio before the end of the text. Goal is to
find general normalizer fixes rather than document-specific patches.

| Segment | Words | Suspected cause |
|---------|-------|-----------------|
| seg20   | 247   | Paragraph 2 contains quoted CLI commands with `!!` and uneven quote stripping — model may bail early on malformed quoted text |
