# Reference-clip-leak examples

VibeVoice regurgitates words from its own voice-clone reference clip's
transcript (Andy's ref clip transcript is the document's own opening
line — see `voices/andy/voice.md`) instead of, or in addition to, the
segment's actual intended text. Confirmed by ear; the QA AlignReport
(forced alignment against the segment's *own* intended text) does not
reliably catch this — see `dev/artifact-hypertext87.md` and
`vibe/listen-test/check-ref-leak.sh` for the detection approach that
does (align a leading slice against the reference clip's own
transcript instead of the segment's text).

| File | Approx. time in audio | Confirmed issue |
|------|------------------------|-----------------|
| `hypertext87-2026-06-26_seg06_seed993445.wav` | ~0.4-7.2s | First ~7s of audio is seg01's opening line ("I'm a Johnny-come-lately...") instead of seg06's actual text. Original discovery case — see `dev/artifact-hypertext87.md` § seg06 — hypertext87-2026-06-26 for the full forced-alignment timing analysis (`Another` forced to span 0.44-2.64s at score 0.42, then a gap until `thing` at 7.15s). Proven via `check-ref-leak.sh`: 2/10 words suspect against the ref-clip transcript vs 9/9 against seg06's own correct text on the same audio slice. No transcript.json survives for this exact take (the file was overwritten by a later resynth before this was investigated) — timing is from the original analysis in `dev/artifact-hypertext87.md`, not re-derived here. |
| `hypertext87-2026-06-27_seg14_seed993445.wav` | ~0-3.6s+ | Confirmed by listening: "I want to mention a couple of numbers, just so that you can size the system. We" — leaked/garbled opening, starting at the very beginning of the segment (`I`=0.01 @0.44s, `want`=0.49 @0.62s, `a`=0.07 @1.64s, `couple`=0.32 @1.82s, `of`=0.00 @2.24s, `numbers,`=0.00 @2.38s). Own-text AlignReport showed 8 low-score words total; ranked near the top of `check-ref-leak.sh`'s candidate list (3/10 suspect against ref-clip text). |
| `hypertext87-2026-06-27_seg06_seed993446.wav` | ~0.4-4.0s+ | Confirmed by listening: "Another thing we should thank Ted for is that he did not just say, 'branch, link, make arbitrary associations.'" — again right from the start (`Another`=0.16 @0.44s, `thing`=0.01 @0.66s, `Ted`=0.01 @1.58s, `is`=0.00 @1.94s, `just`=0.00 @3.20s, `say,`=0.00 @3.32s). Own-text AlignReport shows widespread low-score words across roughly the first half of the segment — unlike the seed=993444 take of the same segment, which had **no signal at all** in transcript/alignment despite an audible problem (see `../truncated/` and `dev/artifact-hypertext87.md`). |

Each `.wav` is paired with its `_report.json`/`_transcript.json` (word-level
timestamps/scores — source of the "approx. time" column above, where
available) and an `_intended-text.txt`/`-text-for-reference.txt` showing
what should have been said and (for the seg06 case) what's believed to
have leaked in, plus `_normalized-text.txt` (what was actually sent to
the TTS engine).
