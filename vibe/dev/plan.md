# Plan: Evaluate vibe voice

## Goal
- To see if we can generate 15-20 minutes of text that Sarah can listen to
  without triggering a migraine .

Steps
1. [x] Section A issues from [dev/normalize-future.md](../dev/normalize-future.md):
   1. [x] Acronym spelling (A1): review existing test coverage for the
      3-letter-acronym rule; add cases for an acronym that should be
      pronounced as a word, and confirm the override map can force
      that behavior over the default spell-out.
   2. [x] Em dash (A2): change `--` handling from "becomes spaces" to
      "becomes a comma" (pause cue), with a unit test. (Reverted back
      to spaces after listen-test — see normalize-future.md section D.)
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
5. [ ] Create audio file for all of `data/authorship.txt` by dividing
   into text segments (~300 words at paragraph division) then
   stitching audio pieces together.
   - [x] Split into 21 segments (`data/authorship_seg01-21.txt`) via
     `split_authorship.py` (250–400 words, paragraph boundaries, headings
     merged into following paragraph)
   - [ ] Seed discovery: run seg07–11 without `--seed`, human listener to pick a voice, find seed in
     `data/runs.jsonl`
   - [ ] Full run: all 21 segments with chosen seed
   - [ ] Stitch segments into final audio file