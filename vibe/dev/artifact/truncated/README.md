# Truncated examples

The audio cuts off or clips mid-content in a way that's audible on
listening, but the AlignReport (forced alignment against the segment's
own intended text) reports completely clean — no filtered or suspect
words at all. Distinct from the `⚠ TRUNCATED` QA flag the existing
pipeline already emits for some segments (e.g. seg14/seg31/seg45 in
`dev/artifact-hypertext87.md`'s seed=993445 table) — those *do* get
flagged; the example here does not, despite being audibly the same
general kind of problem. Like `../garbled/`, this leaves no signal in
transcript/alignment data, so detecting it will likely need an
audio-domain approach.

| File | Approx. time in audio | Confirmed issue |
|------|------------------------|-----------------|
| `hypertext87-2026-06-27_seg06_seed993444.wav` | Not yet pinpointed | Confirmed by listening: truncated/cut off. `_report.json` is fully clean (`{"filtered":[],"suspect":[],"threshold":0.3}`) — zero signal, so unlike the other two categories there's no transcript/score data to derive a timestamp from; finding the exact spot requires listening through the file. |

Paired with its `_report.json`, `_transcript.json`, `_intended-text.txt`,
and `_normalized-text.txt` (what was actually sent to the TTS engine).
