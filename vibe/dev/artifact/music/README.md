# Background-music examples

VibeVoice generates background music under and/or before a word, instead
of (or in addition to) plain speech. Confirmed by ear. Like `../garbled/`
and `../truncated/`, this leaves little to no signal in the AlignReport —
the word it occurs around can still score reasonably even with music
playing underneath.

| File | Approx. time in audio | Confirmed issue |
|------|------------------------|-----------------|
| `hypertext87-2026-06-27_seg49_seed993445.wav` | ~57.6s onward, around/before "webmaster" | Music plays before and over the word "webmaster" (the document's final word — a standalone webmaster-credit line in the source text). `_report.json` is fully clean (`{"filtered":[],"suspect":[],"threshold":0.3}`); the word itself scores 0.53, not flagged as suspect (threshold 0.3). |

Root cause for this specific instance isn't the TTS engine's fault: the
source document's last line is a bare standalone "webmaster" (a leftover
website-footer credit, not part of the actual talk text) — being struck
from the source text and resegmented/resynthesized rather than treated as
a TTS bug. Kept here anyway as a labeled example of "model generates
music" for the case where this happens on text that *should* stay.

Paired with its `_report.json`, `_transcript.json`, `_intended-text.txt`,
and `_normalized-text.txt`.
