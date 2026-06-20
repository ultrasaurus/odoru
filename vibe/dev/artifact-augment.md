# Augment Paper TTS Artifact Review

Test audio: `vibe/data/augment/segment test 0 - 01-05/augment_seg01..05_generated.wav`
(segments 01–05, individual files; not yet stitched into one doc)

**Status: partial review.** Seg01 and seg05 fully reviewed; segs 02–04 not yet
reviewed. The two findings below are the only issues found in segs 01 and 05.

Issues are grouped by type: **TTS artifacts** (hallucinated non-speech audio) vs.
**pronunciation/text-processing issues** (wrong or skipped words).

---

## TTS Artifacts (non-speech audio hallucinated)

| Seg | Timestamp | Description | Trigger text |
|-----|-----------|--------------|---------------|
| 05  | Mid-segment | Music plays before/over the line (not at the leading edge of the segment — occurs partway through) | `Let us consider an augmented architect at work.` |

---

## Pronunciation Issues (wrong rendering of text)

| Seg | Issue | Text | Status |
|-----|-------|------|--------|
| 01  | "I. INTRODUCTION" garbled — heard roughly "a lecture n giawhanor" | `I. INTRODUCTION` | Fixed via `tts_overrides.txt` |
| 01  | "A. GENERAL" garbled — heard roughly "A. engenal" | `A. GENERAL` | Fixed via normalizer: title-case all-caps headings (e.g. "GENERAL" → "General") — same pronunciation to TTS, reads more naturally, and avoids issues when preceded by punctuation like "A." |

---

## Notes

- Seg01–seg05 are fully reviewed; these were the only issues found in them.
- Both seg01 heading issues are fixed: "I. INTRODUCTION" via `tts_overrides.txt`,
  "A. GENERAL" via a normalizer rule that title-cases all-caps headings generally
  (not a one-off override) — likely also fixes the same pattern at other
  section/sub-section headings in this document.
- Music in seg05 occurs mid-segment, not leading-edge — same general class
  (hallucinated non-speech audio) as the leading-edge noise/music artifact in
  [artifact-authorship.md](artifact-authorship.md) seg04, but not the same
  position pattern. Worth tracking whether mid-segment vs. leading/trailing-edge
  position correlates with anything (segment length, sentence boundary, etc.).
