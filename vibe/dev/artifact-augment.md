# AUGMENT Paper TTS Artifact Review

Test audio: `vibe/data/authorship_seg*.wav` (33 segments, stitched)

Issues are grouped by type: **TTS artifacts** (hallucinated non-speech audio) vs.
**pronunciation/text-processing issues** (wrong or skipped words).

---

## TTS Artifacts (non-speech audio hallucinated)

| Seg | Words | Timestamp        | Description      | Trigger text |
|-----|-------|------------------|------------------|--------------|
| 04  | 236   | ~5:22 from start | Background noise | "It also supports the shared-screen, remote collaboration capability discussed below." — leading edge of the architecture section |

Note: this is the middle of the segment -- towards the end, but not at the end

---

## Pronunciation Issues (wrong rendering of text)

All of these can be fixed with `tts_overrides.txt`, except the journal link
pattern (see normalizer note below).

| Seg | Issue | Text |
|-----|-------|------|
| 10  | Parentheses skipped — `(` `)` treated as content delimiters, not read | `if "(" and ")" are set by the author as name delimiters` |
| 14  | Link address mangled — read as "4 b dot L 1" | `link "(4b.l)"` |
| 24  | Link address mangled — `<OAD,2237,>` mispronounced | `as specified in the link "<OAD,2237,>"` |
| 25  | `yyy` read as "why-my" | `"xxx", "yyy", and "zzz" represent Journal item numbers` |
| 27  | "Shared-Screen Conferencing" read as "Shared-Screen Confran-fracing" | Section heading and body |
| 27  | `!` (OK Key) not pronounced — override whole command string | `SMITH!`, `OF12!`, `Viewing (other display)!!` |
| 29  | `printer/graphic port` read as "printer dot graphic port" | `printer/graphic port` |
| 33  | `Oct.` not expanded to "October" | `Oct. 16-18, 1978` |
| 33  | `pp.` not expanded to "pages" | `pp. 63-68` |
| 33  | `CO` (state abbrev.) not expanded | `Denver, CO` |
| 33  | `AFIPS` read as "aye FIPS" | `AFIPS Conference Proceedings` |
| 33  | Journal number 71279 garbled — "seven wan twenty seven nine I aseh lay" | `(AUGMENT,71279,)` |
| 33  | Journal number 14724 garbled — "wan hwanny see apple seven two four" | `(AUGMENT,14724,)` |
| 33  | Contract number garbled — "F three-has-suppo to seventy six C zero zero three" | `Contract F30602-76-C-003` |
| 33  | Journal number 37730 garbled — "three seven seven throar" | `(AUGMENT,37730,)` |

**Normalizer fix needed:** the `NAME,number,` pattern (e.g. `AUGMENT,71279,`) has
no spaces after commas, which confuses TTS. Worth adding a normalizer rule to
expand these into speakable form (e.g. "AUGMENT item 71279") rather than
overriding each instance individually. Standard English always has a space after a
comma, so comma-delimited identifiers without spaces are a general problem class.

---

## Text-Processing Issues (words skipped)

Brackets in the "Text skipped" column were added by the reviewer to mark what
was omitted — they are not present in the source segment files.

| Seg | Words | Position in segment | Text skipped |
|-----|-------|---------------------|--------------|
| 24  | 218   | Mid-segment (first clause of a sentence) | `[A given journal may be set up to serve multiple hosts]` |
| 27  | 255   | Trailing (last paragraph, after first sentence) | `[For instance, Jones can pass control to Smith so that Smith can show him some material or method of work. There are also provisions for the subsequent entry and departure of other conference participants.]` |
| 28  | 255   | Leading edge (section heading fused to first sentence) | `[Embedding the Graphic Illustrations]` — heading before "For complete support..." |

All three are in the normal word-count range (most segments 200–260 words), so
length does not appear to be a factor. Truncations were expected to occur only at
the trailing edge, but seg 28 is at the leading edge — suggesting the skip
mechanism may not be purely trailing-edge. Note: the leading-edge artifact pattern
(noise, music) is a separate phenomenon from text truncation.

**Fix approach:** AlignReport (forced-alignment) can potentially detect skips
automatically. Detection is not guaranteed to be reliable, but when a skip is
found the fix is a full re-render of the segment (no alternative — stitching
a separately synthesized clip doesn't work because voice varies slightly between
synthesis runs even with the same seed; see vibevoice.md).

---

## Notes

- **Seg 25: 376 words** — longest segment by a significant margin (next is seg 14
  at 338, most others 200–260). Reviewer observed "fast voice"; segment length is
  a plausible cause and worth investigating.
- `!` in commands should be verbalized as "exclamation point"; plan is to override
  the whole command string in `tts_overrides.txt`.
- Review stopped at Ref-4 in Seg 33; Ref-5 through Ref-8 not yet verified.
- Fixes proposed by reviewer: add months/states/`pp.`/`AFIPS` to `tts_overrides.txt`;
  suppress or expand parenthesized name-delimiter examples in seg 10.
