# Known TTS Failure Cases

Wav+text pairs with confirmed artifacts, truncation, or hallucination.
Use these to reproduce and validate normalizer fixes.

---

## Truncation

Model stops generating before the end of the text.

### seg07 — too long (~400 words)

- **Wav**: `data/segments-thru-Jun18/authorship_seg07_generated.wav`
- **Text**: `data/segments-thru-Jun18/authorship_seg07.txt`
- **GPU**: RTX 3090 (24GB)
- **Symptom**: Audio clipped at ~144s, several sentences missing from end
- **Cause**: Segment too long (~400 words). Fixed by resplitting from seg12
  onward at 150–250 words.
- **Status**: Not re-run. Superceded by shorter seg12–16 covering same content.

### seg20 — malformed quoted text

- **Wav**: `data/segments-thru-Jun18/authorship_seg20_generated.wav`
- **Text**: `data/segments-thru-Jun18/authorship_seg20.txt`
- **GPU**: RTX A4000 (16GB)
- **Symptom**: Truncated mid-paragraph-4 ("so that Smith can show him some
  mat[erial]...")
- **Cause**: Paragraph 2 contains quoted CLI commands with `!!` and uneven
  quote stripping — model may bail early on malformed quoted text. Low VRAM
  (16GB) may also be a factor.
- **Status**: Unfixed. Needs re-run on >=24GB GPU to isolate VRAM vs. content
  cause. General normalizer fix for `!!` and unbalanced quotes TBD.

---

## Hallucination / Repeated-Phrase Skip

Model hallucinates or skips ahead when text contains repeated similar phrases.

### seg09 — repeated phrases on 16GB GPU

- **Wav**: `data/segments-thru-Jun18/authorship_seg09_generated.wav`
- **Text**: `data/segments-thru-Jun18/authorship_seg09.txt`
- **GPU**: RTX A4000 (16GB), pod ifn74uazcr7nci
- **Symptom**: Model hallucinated or skipped content at sections with similar
  repeated phrasing
- **Cause**: VRAM-related (16GB cramped). Same content ran clean on 48GB GPU.
- **Status**: Not re-run on >=24GB GPU yet. `new-pod` now enforces >=24GB
  minimum, so a fresh run should be clean.

### seg10 — same issue, clean re-run available

- **Bad wav**: `data/segments-thru-Jun18/authorship_seg10_generated.wav`
- **Clean wav**: `data/2026-06-18 9-30a/authorship_seg10_generated.wav`
- **Text**: `data/segments-thru-Jun18/authorship_seg10.txt`
  (also at `data/2026-06-18 9-30a/authorship_seg10.txt`)
- **GPU (bad run)**: RTX A4000 (16GB)
- **GPU (clean run)**: RTX A6000 (48GB)
- **Symptom**: Bad run has hallucinated/repeated phrases. Clean re-run is fine.
- **Status**: Resolved by >=24GB VRAM enforcement.

---

## Vocalization Artifacts from `--`

Space-padded double-hyphens (` -- `) produced audible vocalization artifacts
between words. Fixed in normalizer Pass 3 (` -- ` → `, `).

### seg14 — "tree fallack and" artifact

- **Wav**: `data/segments-thru-Jun18/authorship_seg14_generated.wav`
- **Text**: `data/segments-thru-Jun18/authorship_seg14.txt`
- **GPU (artifact run)**: RTX A4000 (16GB), pod tttf224qevgzyt
- **Symptom**: "Statement 3c -- and" → spoken as "Statement 3c tree fallack and"
- **Status**: Fixed. Current wav in `segments-thru-Jun18/` is the clean re-run
  (pod hst1oarkuu513x, RTX 3090 24GB) after em-dash fix. Original artifact wav
  was overwritten.

### seg15 — "lines -- things" artifact

- **Wav**: `data/segments-thru-Jun18/authorship_seg15_generated.wav`
- **Text**: `data/segments-thru-Jun18/authorship_seg15.txt`
- **GPU (artifact run)**: RTX A4000 (16GB), pod tttf224qevgzyt
- **Symptom**: "lines -- things" produced vocalization artifact
- **Status**: Fixed. Same re-run as seg14. Current wav is clean.

---

## Dialog Format Artifacts

Reformatting as Speaker 1 / Speaker 2 dialog made artifacts worse on short
imperative sentences, not better.

### seg07 dialog experiment

- **Wav**: `data/segments-thru-Jun18/authorship_seg07_dialog_generated.wav`
- **Text**: `data/segments-thru-Jun18/authorship_seg07_dialog.txt`
- **Symptom**: Widespread glitches on short imperative lines (e.g. "Move
  Branch 2b.") when split across dialog turns
- **Lesson**: Dialog format does not help short-sentence artifacts; splitting
  short fragments into the following paragraph (as done in
  `split_authorship*.py`) is the effective fix.
