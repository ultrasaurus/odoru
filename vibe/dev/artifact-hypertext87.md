# Hypertext '87 Keynote TTS Artifact Review

Test audio: `vibe/data/andy/hypertext87-2026-06-22/hypertext87_seg*_generated.wav`
(10 of 51 segments synthesized so far), Andy voice, seed 993445, speed 0.95,
cfg_scale 1.3 (default), RunPod v17/v18 images.

Issues are grouped by type: **TTS artifacts** (hallucinated/wrong audio) vs.
**stitching artifacts** (concat-boundary effects, not present in individual
segment wavs).

---

## TTS Artifacts

| Seg | Description    | Notes |
|-----|----------------|-------|
| 07 hypertext87-2026-06-22  | Hallucinated repeat — audio contains text that belongs to seg01, not seg06's own source text | Reported by listening; not caught by forced-alignment QA (`hypertext87_seg06_report.json` came back clean both times, and the transcript shows the *intended* text aligned cleanly with high scores — forced-alignment timestamps the given text against the audio, it does not independently transcribe what was actually spoken, so it can't catch this class of error). Distinct from the known same-segment repeated-phrase hallucination in `dev/quirks.md`/`dev/failures.md` (those are *within* one segment's own text looping; this is cross-segment — audio from an unrelated, non-adjacent segment leaking in). **2026-06-26 — retracted lead:** initially hypothesized this was specific to the new concurrent Cloud Run batch path (since seg06 sounded clean on relisten in the original RunPod sequential run, `odoru-vibe-wt/vibe/data/andy/hypertext87-2026-06-22`). **That hypothesis is wrong** — verified `hypertext87_seg06_generated.wav` in that directory has a file mtime from its original generation and was never regenerated, so it's the exact same audio that was first reported as hallucinating. The "now sounds clean" / "originally hallucinated" reports are about the same byte-identical file, so this is not a sequential-vs-batch code-path difference. Real explanation unknown — possibilities include misidentifying which segment/run had the issue on first listen, or a genuinely subtle artifact that's easy to miss/unmiss between listens. No reliable reproduction conditions established; back to square one on root cause. |
| 01  | Trailing "s" clipped at the very end of the segment's last word | Heard on multiple re-renders across different seeds/configs, including the seed=993445/speed=0.95 version. Likely a genuine generation-boundary artifact (model cuts before fully voicing the final consonant) rather than something fixable via overrides/normalizer/fade — fading only smooths the cut, it doesn't restore the missing sound. No fix attempted; flagged as a probably-inherent VibeVoice limitation. |

---

## seg06 — hypertext87-2026-06-26

Confirmed, separate from the seg07/hypertext87-2026-06-22 case above — this
is the resegmented (49-segment, headings-stripped) run, file
`vibe/data/andy/hypertext87-2026-06-26/hypertext87_seg06_generated.wav`.

There's a hidden anomaly in the timing data that the report's
`clean`-looking summary doesn't surface. From
`hypertext87_seg06_transcript.json`:

```
Another         start=0.44 end=2.64 dur=2.20 score=0.424
thing           start=7.15 end=7.31 dur=0.16 score=0.999
```

"Another" is forced to span 2.2 seconds (vs ~0.2s for every normal word
after it) at a low 0.42 score, and then there's a 4.5-second gap between
"Another" ending at 2.64s and "thing" starting at 7.15s — nothing in the
given text accounts for that stretch. Forced alignment is monotonic and
must assign every bit of audio to some word in the given sequence, so it
dumped that whole unaccounted region onto "Another" rather than flagging
it as unmatched. From "thing" onward (7.15s+), every word aligns cleanly
at ~0.94–0.999 confidence — completely normal pacing.

So the first ~7 seconds of `hypertext87_seg06_generated.wav` contains
audio that doesn't correspond to any word in
`hypertext87_seg06_normalized.txt`. By ear, the content in that gap is
seg01's opening line ("I'm a Johnny-come-lately to hypertext: I didn't get
started until 1967...") — confirmed by listening, though the exact wording
transcribed here was copied from `seg01_normalized.txt` rather than typed
fresh from listening, so don't read anything into the specific
normalized-vs-raw phrasing (e.g. "I am A Johnny Come Lately" vs "I'm a
Johnny-come-lately") — that's an artifact of how the note was written, not
a confirmed finding about which text variant leaked.

This is a confirmed real cross-segment leak (not a QA tool gap, not a
mishearing, not the earlier VSCode-JSON-pretty-print red herring that
briefly looked like the same thing). Points at a real bug somewhere in the
synthesis path — either two datasets/files got mixed up on disk, or
there's an actual code bug in how the batch path (this run went through
the Cloud Run `/batches` concurrent path, see `dev/listen-test-batch.md`)
assembles per-segment audio. Worth investigating with the actual
synthesis/batch code now that there's a concrete, reproducible 7-second
window and exact leaked text to search for.

---

## hypertext87-2026-06-27 (49-segment, seed=993445, cfg=1.3, speed=0.95)

Full from-scratch resynth after the splitter/segmenter fixes and the
general-digit-spelling normalizer fix. Listened through the whole batch.
Findings below, three distinct failure signatures (see discussion in the
2026-06-27/28 session — text-domain QA misses two of these three
entirely):

| Seg | Description | QA/alignment signal |
|-----|-------------|----------------------|
| 05 | Garbled around "Well, I've used that bon mot ever since" and again around "and I think he is absolutely right: displays are the way to go" | Mixed — `report.json` does flag `"I've"`(0.14) `"used"`(0.15) `"that"`(0.10) as low-score near the first spot, but the words around "absolutely right: displays are the way to go" all score 0.97-0.999, despite reported garbling there. |
| 06 | Heard "Another thing we should thank Ted for is that he did not just say, 'branch, link, make arbitrary associations.'" as a ref-clip-style problem | **No signal at all** — `report.json` clean, transcript shows a single clean occurrence, full expected word count (202) and duration (78.4s), no other segment's transcript contains a stray copy of this text either. Worth noting seg06 was also the segment that hallucinated the literal Andy reference-clip transcript in the earlier seed=993444 runs (see seg06 section above) — same segment, different seed, a difference-looking symptom this time, and this time invisible to every text-based check we have. |
| 14 | Reference-clip leak — "I want to mention a couple of numbers, just so that you can size the system. We" | Matches our ranked candidate list from `check-ref-leak.sh` (own-text: 8 low-score words; ref-clip slice test: 3/10 suspect, near the top of the ranking). |
| 20 | Reported as garbled — "And then we would play peek-a-boo, strip off the first overlay" | High-confidence alignment throughout (`peek`=0.66, `boo,`=0.99, `strip`=0.98, `overlay`=0.97) — QA reported only 1 unrelated low-score word and 4 filtered ellipsis tokens elsewhere in the segment; this spot wasn't flagged at all. |

Text-side fixes already applied to `tts_overrides.txt` for unrelated
issues found during this same listen: `CD ROMs` → `C D Roms` (was being
read as Roman numeral CD = "four hundred"), `U.S.` → `U S` (was being
read as "U dot S" via the identifier-dot-replacement pass misfiring on
this abbreviation).

These three signatures argue that any further detection needs to look at
the audio itself (spectral/energy/pitch discontinuities), not just
transcript alignment — segments 06 and 20 above transcribe and time
*perfectly*, so no text-domain check could ever flag them by construction.

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
- Seg07-hypertext87-2026-06-22 cross-segment hallucination: defer investigation
  into whether `vibe-service`'s VibeVoice inference process retains
  state (cache, KV-cache, prompt buffer) across `synthesize` calls within one
  pod's lifetime that could leak content between unrelated segments. Not yet
  resolved as of this writing.
- See `dev/voices.md` for the Andy seed-selection notes (993445 preferred,
  speed 0.95) that this run is built on.
