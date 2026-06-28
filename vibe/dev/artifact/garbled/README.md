# Garbled-but-well-timed-speech examples

**Pattern to watch for when reviewing normalized text:** heavy
punctuation runs (e.g. a spaced ellipsis followed by a closing quote and
period — `. . . ".`) and foreign/technical phrases (e.g. "bon mot") are
the common thread across these examples so far. Title-Case words are an
extra red flag — the normalizer tends to treat Title Case as a sign of a
proper noun and leaves it alone, even when it's actually a foreign phrase
or technical term that needs phonetic help, not name-preservation. When
reviewing a document for likely trouble spots before synthesizing, scan
for both of these rather than waiting to catch them by ear afterward.
Fixes so far went into `tts_overrides.txt`: `. . . "` → `,` and
`bon mot` → `Bohn Moh`.

A stochastic TTS generation glitch — the audio is audibly mangled/garbled
in a specific spot, but forced alignment (against the segment's own
correct intended text) often times those exact words with *high*
confidence anyway, since CTC-style alignment doesn't strongly penalize
poor articulation as long as the rough timing/phonemes are plausible.
This is a different and in some ways harder failure mode than
`../ref-clip-leak/` — it leaves little to no signal in transcript or
alignment-score data, so any detector for this class will likely need to
look at the audio itself (spectral/energy/pitch discontinuities), not
text-domain QA. See discussion in `dev/artifact-hypertext87.md` §
hypertext87-2026-06-27.

Legend: Garbled text is shown with angle brackets around <intended text> or <...> if the garbled speech is inserted noise

| File | Approx. time in audio | Confirmed issue |
|------|------------------------|-----------------|
| `hypertext87-2026-06-27_seg05_seed993445.wav` | ~40.6-42.6s, again ~43.9-46.3s | Garbled "<Well, I've used that bon mot> ever since" (~40.65-42.63s; *is* flagged low-score: `Well,`(0.09) `used`(0.15) `that`(0.10) `bon`(0.38) `mot`(0.31)) and again "and I think he is <absolutely right: displays are the way to go>" (~43.87-46.30s; **not** flagged — those words score 0.97-1.00). |
| `hypertext87-2026-06-27_seg20_seed993445_take1.wav` | ~55.6-57.5s | First take, seed=993445. "But wait there's more. <...> And then we would play peek-a-boo, strip off the first overlay." High-confidence alignment throughout that exact region (`peek`=0.66 @55.58s, `boo,`=0.99 @55.90s, `strip`=0.98 @56.14s, `overlay`=0.97 @57.12s) — **no signal at all** in the AlignReport for this spot. |
| `hypertext87-2026-06-27_seg20_seed993444_take2.wav` | ~53.9-56.5s | Second take, seed=993444 (after a voice-mixup retry — see session notes 2026-06-28). Confirmed garbled again in the same region on relisten "But wait there's more <...> And then we would play peek-a-boo, strip off the first overlay.", despite a different seed. Same high-confidence-despite-garbled pattern (`peek`=0.83 @53.90s, `boo,`=0.93 @54.24s, `strip`=0.93 @55.04s, `overlay`=0.99 @56.08s). |

Each `.wav` is paired with its `_report.json` and `_transcript.json` (word-level
timestamps/scores — source of the "approx. time" column above) where
available, plus `_intended-text.txt` (raw source text) and
`_normalized-text.txt` (what was actually sent to the TTS engine).
