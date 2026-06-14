# Normalizer: future fixes

Found by diffing `normalize()` output for `data/markers.txt` against
the source text (see `tts/examples/normalize_dump.rs`) and listening
to the generated audio.

## A. Verify with unit tests first
After units test past, verify by listening with authorship.txt sections
- **Limit acronym letter-splitting to 3-letter acronyms.** "SID" ->
  "S I D" is fine, but blanket letter-splitting of all-caps words is
  too broad — longer all-caps words are rarely pronounced as letters.
  Scope the rule to 3-letter acronyms, with an override mechanism for
  exceptions. 
  FYI: this was intended behavior. Do we need more test cases?
- **Em dash "--" should not become "or".** `4b.dt" -- "or"` is wrong —
  "--" should become a pause (or "to", depending on context), not
  literally "or".
- **Detect ref/code patterns and fix normalization.** Patterns like
  `<Ref-1.l>`, `<Ref-1.l:i;LL>`, `(4b "*D" .l)`, `<OAD,2237,>`,
  `(DDD,xxx,bb)` (number.letter refs, angle-bracket tags,
  comma-separated codes) aren't handled by the current normalizer and 
  could be — these look like a detectable pattern (punctuation +
  short alphanumeric tokens) that get garbled and likely need spaces
  between letters/numbers to be pronounced correctly.

## B. Need interactive testing
- **Test "Move" -> "moove" as a verb.** The respelling was needed for
  "move" in a list of commands (imperative/command-name usage). Seems
  harmless when "move" is used as a regular verb, but worth testing
  more sentences to confirm it doesn't sound wrong there.
- **Test alphanumeric ID spacing more broadly.** "3b5" -> "3 b 5",
  "4b" -> "4 b" sounds fine in this case; worth testing against file
  names/version numbers elsewhere to confirm it generalizes.

## Confirmed working as intended (no action needed)
- Hyphenated compounds losing the hyphen ("cursor-selected" ->
  "cursor selected") — sounded off in F5 without this, fine in Vibe.
- "NULL" -> "null" — intended.
