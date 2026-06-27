# Voice Notes

## sarah/ref.wav

cfg_scale=1.3, seed=71463

### testing details
segments authorship_seg07–11, 2026-06-17

| Seed  | Notes |
|-------|-------|
| 42831 | Pedantic and a bit harsh |
| 17654 | Good, clear, slightly pedantic, nice range of pitch |
| 93017 | Good, soft and slow (maybe more sleep-inducing than 17654) |
| 55289 | Good, maybe a little high and slightly pedantic |
| 71463 | **Preferred.** More energy, smooth, nice range of pitch, maybe a little fast (but might be good for reading efficiently) |

Seeds 17654, 93017, 55289 are acceptable as additional speakers for
multi-speaker experiments (e.g. splitting long quoted passages).

## andy/ref.wav

cfg_scale=1.3, speed=0.95, seed=993444

consider seed=993445 which also sounds good (switched for artifacts, but could be just random)

### testing details
segments hypertext87_seg01–05, 2026-06-24

| Seed   | Notes |
|--------|-------|
| 993445 | **Preferred.** Good |
| 937392 | High and breathy |
| 635735 | Started good, but got sing-songy and squeaky |
| 219470 | Good, even, just fast |
| 693262 | Very fast and a little high |

993445 also tested at `--speed 0.95` (segments seg04–05, re-run with this
seed) to slow down the "fast" voices — sounded good, evened out the pacing
without other artifacts.