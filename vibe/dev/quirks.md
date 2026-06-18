# VibeVoice TTS Quirks

Observations about model behavior that are not (yet) addressed by the
normalizer or overrides. Goal is to accumulate enough data to find general
fixes.

## Audio Truncation

Model stops generating before the end of the text.

| Segment | Words | GPU          | VRAM  | Notes |
|---------|-------|--------------|-------|-------|
| seg07   | ~400  | RTX 3090     | 24GB  | Truncated; suspected cause: segment too long. Shortened to 150-250 words for subsequent segments. |
| seg20   | 247   | RTX A4000    | 16GB  | Truncated mid-paragraph-4. Paragraph 2 has quoted CLI commands with `!!` and uneven quote stripping — model may bail early on malformed quoted text. |

**Hypothesis:** seg07 truncation was likely length-related (144s audio); seg20
truncation may be content-related (malformed quoted text) or VRAM-related
(cramped 16GB card). Need more data — track GPU for future truncations.

## Short Sentence Artifacts

Very short sentences (single commands, fragments) produce audio glitches,
extra vocalizations, or garbled output. Observed in seg07 content with
imperative one-liners like "Move Branch 2b." Attempted fix via dialog
format (Speaker 1 / Speaker 2 split) made artifacts worse.

The `split_authorship*.py` scripts merge headings and short fragments into
the following paragraph before segmenting, so short lines rarely appear as
standalone `Speaker 1:` entries. This appears to have largely resolved the
issue in practice — no short-sentence artifacts reported in seg12–25.
However the fix is implicit in the splitting logic, not the normalizer, so
it could resurface in documents where short sentences appear mid-paragraph
or are unavoidable at paragraph boundaries.

## Voice Shift Between Segments

Noticeable shift in tone/prosody at segment boundaries. Using the same seed
(71463) throughout makes transitions sound like "a real human just shifting
speech after a breath" — acceptable for paragraph-boundary breaks. Breaks
mid-paragraph would likely be more jarring.

## GPU Performance vs. VRAM

RTF varies significantly by GPU. Higher VRAM cards are noticeably faster:

| GPU           | VRAM  | Typical RTF |
|---------------|-------|-------------|
| RTX A5000     | 24GB  | 0.29–0.30x  |
| RTX 3090      | 24GB  | 0.29–0.40x  |
| RTX A6000     | 48GB  | 0.36x       |
| RTX A4000     | 16GB  | 0.45–0.87x  |

**Conclusion**: 24GB+ VRAM is the threshold for good performance. The A4000
(16GB) is significantly slower and may contribute to hallucinations. The
>=24GB VRAM filter is now the default in `new-pod`.

Hypothesis: cramped VRAM (16GB) may also contribute to truncation, but
seg07 truncated on a 3090 (24GB), so VRAM alone does not explain it.

## Repeated/Similar Phrases

seg09/seg10 content has repeated similar phrases that caused the model to
hallucinate or skip ahead. Observed on RTX A4000 (16GB). Re-run on RTX
A6000 (48GB) with >=24GB VRAM filter — **no hallucination**. Confirmed
VRAM-related, not content-related. Fixed by enforcing 24GB+ minimum in
`new-pod`.

## Punctuation Issues

Punctuation that the model vocalizes unexpectedly or misreads:

| Pattern | Behavior | Status |
|---------|----------|--------|
| ` -- ` (double hyphen / em dash) | Extra space becomes audible vocalization between words | **Fixed** in normalizer: replaced with `, ` |
| `...` / `…` (ellipsis) | Caused "taunt" mispronunciation when converted to newline; orphaned ` ,` → "maht" when leading space not stripped | **Fixed**: ellipsis stripped with leading space (`" ..."` → `""`) |
| `"."` (quoted period) | Model reads standalone quoted punctuation strangely | **Fixed** via override: `"." → "Baseline dot symbol"` |
| `":"` (quoted colon) | Spoken incorrectly when quoted | **Fixed** via override: `":"` → `"colon"` |
| `!!` (multiple exclamation points) | In seg20 quoted CLI commands — may confuse model, possibly contributing to truncation | **Unfixed** — no general rule yet |
| Quoted text ≤5 words | Short quoted phrases cause TTS mangling | **Fixed**: stripped by `strip_short_quotes` pass |
| Uneven quote stripping | Mismatched quotes left after short-quote stripping (e.g. seg20 line 2) | **Unfixed** — quote stripping doesn't track open/close balance |

## Segment 26 (References) — Fast RTF

References section (seg26, 266 words) produced 204.8s audio at RTF 0.22x —
much faster than body segments. Likely because reference entries are short,
formulaic sentences with lots of proper nouns and numbers that the model
processes quickly.
