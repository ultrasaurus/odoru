# F5-TTS Parameter Experiments

Test audio: `data/abstract.txt` (1:11 audio) unless noted.  
Reference voice: `ref.wav` (Sarah).  
Hardware: M1 Mac, 16GB unified memory.  
Backend: `f5-tts-mlx`, 4-bit quantized (~363MB).

---

## steps / method

Controls diffusion quality vs. generation speed.  
`steps` = number of ODE solver steps. `method` = solver algorithm.

| steps | method | audio | generated | realtime | notes |
|-------|--------|-------|-----------|----------|-------|
| 8 | rk4 | 1:11 | 6:50 | 0.17x | **Baseline. Best quality. Kept as default.** |
| 6 | rk4 | 1:11 | 4:59 | 0.24x | Better than steps=4 but not good enough |
| 8 | euler | 1:11 | 1:40 | 0.70x | Hisses too much |
| 4 | euler | 1:11 | 0:46 | 1.53x | Haunted robot — unusable |

**Conclusion:** `steps=8, method="rk4"` is the quality floor. Neither reducing
steps nor switching to euler produces acceptable results on this voice/content.

---

## cfg_strength

Classifier-free guidance strength. Higher = closer to reference voice character.

| cfg_strength | notes |
|-------------|-------|
| 2.0 | Original default. Prior run — not bothering for now, will revisit. |
| 1.5 | **Preferred for Sarah. Good voice character without being too close.** |
| 1.0 | Sounds a bit wobbly for this voice. |

---

## speed

Speech rate multiplier. Tuned per-voice, not per-request (stored on `Voice`).

| speed | notes |
|-------|-------|
| 1.0 | Original default. Too fast. |
| 0.85 | **Preferred. Sounds natural for this voice.** |

---

## Longer content

| file | audio | generated | realtime | notes |
|------|-------|-----------|----------|-------|
| abstract.txt | 1:11 | 6:50 | 0.17x | Dense academic sentences |
| authorship1.txt | 8:49 | 87:37 | 0.10x | Full article, longer sentences |

Generation time scales with output duration and sentence complexity — longer,
more phonetically complex sentences take disproportionately longer. Both runs
are firmly in "go do something else" territory, which is acceptable for
batch generation.

---

## Current defaults

```rust
Voice::f5(
    "scott",
    "ref.wav",
    "It may contain summaries or maps of its content and \
     their interrelations. It may contain annotations.",
)
// Voice::f5() sets: speed = 0.85, cfg_strength = 2.0 (default: sound like reference)
// Sarah voice overrides to 1.5

// tts.py synthesis call:
// steps=8, method="rk4", sway_sampling_coef=-1.0
```

---

## Notes

- Generation time is not purely proportional to text length — sentence
  complexity (phoneme difficulty, unusual terms) affects it significantly.
- Parallelising across multiple workers is the main remaining speedup lever.
  Each worker requires ~363MB of RAM; 2 workers is the practical limit on
  16GB M1.
- Client-side playback speed (Web Audio API `playbackRate`) is the right
  way to let users adjust listening speed — no re-synthesis needed.

---

## Parallelism (workers)

Tested with first 3 sentences of abstract (20.1s audio).

| workers | generated | realtime | notes |
|---------|-----------|----------|-------|
| 1 | 1:59 | 0.17x | Baseline |
| 2 | 2:03 | 0.16x | Slightly slower |

**Conclusion:** Multiple workers provide no benefit on M1. Unified memory means
both workers compete for the same memory bandwidth, and MPS is already saturated
by a single worker. Adding a second worker adds contention, not compute.

The `workers` field is retained in `Backend::F5Tts` for future use on hardware
with dedicated GPUs (cloud deployment), where each worker would have its own
compute and memory.
