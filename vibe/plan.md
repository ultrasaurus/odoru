# Plan: Evaluate vibe voice

## Goal
- To see if we can generate 15-20 minutes of text that Sarah can listen to
  without triggering a migraine .

Steps
1. Section A issues from [dev/normalize-future.md](../dev/normalize-future.md):
   1. Acronym spelling (A1): review existing test coverage for the
      3-letter-acronym rule; add cases for an acronym that should be
      pronounced as a word, and confirm the override map can force
      that behavior over the default spell-out.
   2. Em dash (A2): change `--` handling from "becomes spaces" to
      "becomes a comma" (pause cue), with a unit test.
   3. Ref/code patterns (A3): Sarah to draft test cases from
      `authorship.txt` (may span sections beyond Markers); design and
      implement once cases are in hand.
2. Listen test: create audio wav files for sections of
   `data/authorship.txt`:
    * Markers
    * Traveling Through the Working Files
    * Supporting Multi-Party Collaboration
3. Write new test sentences that form a reasonably realistic prose
   paragraph covering Section B in
   [dev/normalize-future.md](../dev/normalize-future.md), create
   audio, listen test.
4. Create audio file for all of `data/authorship.txt` by dividing
   into text segments (~300 words at paragraph division) then
   stitching audio pieces together.
   (Needs design when we get there — how segments are split/joined,
   tooling, etc.)