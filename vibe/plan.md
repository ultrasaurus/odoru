# Plan: Evaluate vibe voice

## Goal
- To see if we can generate 15-20 minutes of text that Sarah can listen to
  without triggering a migraine .

Steps
1. Write failing unit tests for the Section A issues listed in
   [dev/normalize-future.md](../dev/normalize-future.md), then fix
   `normalize()` until they pass.
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