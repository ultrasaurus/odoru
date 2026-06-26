# Hypertext '87 Keynote TTS Artifact Review

Test audio: `vibe/data/andy/hypertext87-2026-06-22/hypertext87_seg*_generated.wav`
(10 of 51 segments synthesized so far), Andy voice, seed 993445, speed 0.95,
cfg_scale 1.3 (default), RunPod v17/v18 images.

Issues are grouped by type: **TTS artifacts** (hallucinated/wrong audio) vs.
**stitching artifacts** (concat-boundary effects, not present in individual
segment wavs).

---

## TTS Artifacts

| Seg | Description | Notes |
|-----|-------------|-------|
| 06  | Hallucinated repeat — audio contains text that belongs to seg01, not seg06's own source text | Reported by listening; not caught by forced-alignment QA (`hypertext87_seg06_report.json` came back clean, and `hypertext87_seg06_transcript.json` shows the *intended* text aligned cleanly with high scores — forced-alignment timestamps the given text against the audio, it does not independently transcribe what was actually spoken, so it can't catch this class of error). Distinct from the known same-segment repeated-phrase hallucination in `dev/quirks.md`/`dev/failures.md` (those are *within* one segment's own text looping; this is cross-segment — audio from an unrelated, non-adjacent segment leaking in). Investigation spun off separately (see Notes below). |
| 01  | Trailing "s" clipped at the very end of the segment's last word | Heard on multiple re-renders across different seeds/configs, including the seed=993445/speed=0.95 version. Likely a genuine generation-boundary artifact (model cuts before fully voicing the final consonant) rather than something fixable via overrides/normalizer/fade — fading only smooths the cut, it doesn't restore the missing sound. No fix attempted; flagged as a probably-inherent VibeVoice limitation. |

---

## Stitching Artifacts (fixed)

| Issue | Fix |
|-------|-----|
| Hard concat (`ffmpeg -f concat`, no gap) pops at every segment boundary — VibeVoice doesn't fade to silence at the end of a segment, so each cut is audible | Added `listen-test/stitch.sh`: fades each segment in/out (150ms default) and inserts an 800ms silence gap between segments before concatenating. See `dev/listen-test.md` § 4. The seg01 "s" clip (above) persists even with the fade — the fade smooths the *cut*, not a missing phoneme. |

---

## Notes

- The source text itself has a known issue the reviewer plans to fix in a
  separate session; fixing it will require resegmenting and resynthesizing
  the whole document, so the current 10-segment partial run (and any
  artifacts logged above tied to specific segment boundaries/numbering) will
  be discarded once that's done. This doc is mainly a record of *infrastructure*
  findings (hallucination, clipping, stitching) that should carry over to the
  next run, not a final QA pass on this text.
- Seg06 cross-segment hallucination: background investigation task spun off
  to check whether `vibe-service`'s VibeVoice inference process retains
  state (cache, KV-cache, prompt buffer) across `synthesize` calls within one
  pod's lifetime that could leak content between unrelated segments. Not yet
  resolved as of this writing.
- See `dev/voices.md` for the Andy seed-selection notes (993445 preferred,
  speed 0.95) that this run is built on.
