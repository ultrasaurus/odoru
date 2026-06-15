# Normalizer: future fixes

Found by diffing `normalize()` output for `data/markers.txt` against
the source text (see `tts/examples/normalize_dump.rs`) and listening
to the generated audio.

## A. Verify with unit tests first
After units test past, verify by listening with authorship.txt sections
- ✅ **Limit acronym letter-splitting to 3-letter acronyms.** "SID" ->
  "S I D" is fine, but blanket letter-splitting of all-caps words is
  too broad — longer all-caps words are rarely pronounced as letters.
  Scope the rule to 3-letter acronyms, with an override mechanism for
  exceptions.
  FYI: this was intended behavior. Added more test cases.
- ✅ **Em dash "--"**: tried changing from spaces to "," for a pause cue,
  but listening to the multiparty section showed several `--` occurrences
  produced hallucinated/garbled words ("prawned", "prawnnd") in VibeVoice.
  Reverted back to spaces (each `-` of `--` becomes a space, as before).
- ✅ **Detect ref/code patterns and fix normalization.** Patterns like
  `<Ref-1.l>`, `<Ref-1.l:i;LL>`, `(4b "*D" .l)`, `<OAD,2237,>`,
  `(DDD,xxx,bb)` (number.letter refs, angle-bracket tags,
  comma-separated codes) aren't handled by the current normalizer and
  could be — these look like a detectable pattern (punctuation +
  short alphanumeric tokens) that get garbled and likely need spaces
  between letters/numbers to be pronounced correctly.
  - `(4b "*D" .l)`, `<OAD,2237,>`, `(DDD,xxx,bb)` already normalize
    acceptably (checked via `normalize()`).
  - `<Ref-1.l>` and `<Ref-1.l:i;LL>` (the only two occurrences in
    `authorship.txt`, lines 113 & 173) weren't handled by `<Tag-N>`
    expansion (suffix isn't all-digit) — added as global overrides in
    `tts_overrides.txt`, with a TODO to migrate to per-document
    overrides once that mechanism exists.

    * The point to be made here is that with the link "<Ref-1.l>", I can reference the original source document.
    * The link "<Ref-1.l:i;LL>" points to the document referenced by the link in the statement named "Ref-1", invoking viewspec "i" for user content filtering, and sets the filter to "LL" to show only those statements beginning with a lower-case letter.
    * The path for '(4b "*D" .l)' would be "to 4b, scan for first occurrence of "*D", then follow the next link found in that statement."
    * as specified in the link "<OAD,2237,>".  <-- fine for unit test, full sentence below for listening test
    * The system assigns a straightforward accession identifier (a simple number), and any authorized worker is henceforth guaranteed access to that Journal item by specifying the name of the Journal-collection and the Journal-item number -- e.g., as specified in the link "<OAD,2237,>".
    * Frankly, John, I think your comment in (DDD,xxx,aa) is a mistake! Didn't you notice the earlier assumption in (DDD,xxx,bb)? Maybe you should go back to Tom's earlier requirements document -- especially at (EEE,yy,cc)." (Here, "DDD" and "EEE" represent Journal names, "xxx", "yyy", and "zzz" represent Journal item numbers, and "aa", "bb", and "cc" represent addresses pointing to specific passages in those Journal files.)

## B. Need interactive testing
- **Test "Move" -> "moove" as a verb.** The respelling was needed for
  "move" in a list of commands (imperative/command-name usage). Seems
  harmless when "move" is used as a regular verb, but worth testing
  more sentences to confirm it doesn't sound wrong there.
- **Test alphanumeric ID spacing more broadly.** "3b5" -> "3 b 5",
  "4b" -> "4 b" sounds fine in this case; worth testing against file
  names/version numbers elsewhere to confirm it generalizes.

- The following paragraph captures some cases in sentences:

  The dancer's movement was beautiful. I want to move closer to see well.
  The next scene stated with an abrupt transition; I'd like to delete it.
  Move over so I can get by. I think there needs to be a v2 of this
  performance, though the dance was so good, I'd attend version 1.1.

  Plese come visit me sometime. I'm on the top floor: apartment 14B. My
  neighbor in 14C loves 1960's classic movies.

- Are there cases when alpha numerics would be pronounced
  without spelling thme out?

## C. Number pronunciation — listen for these

- ✅ **Leading-zero IDs** ("0609", "069", "012", "0306") now read digit by
  digit ("0 6 0 9") instead of being misread (e.g. "zero b ol nine").
  Listen for these in the "Markers" and "Traveling Through the Working
  Files" sections.
- ✅ **Comma-grouped numbers** ("2,000", "100,000") now expand to words
  ("two thousand", "one hundred thousand") via the `num2words` crate,
  instead of being read as separate digit groups around the comma
  ("two thou zero"). "2,000" appears near the start of `authorship.txt`.

## Confirmed working as intended (no action needed)
- Hyphenated compounds losing the hyphen ("cursor-selected" ->
  "cursor selected") — sounded off in F5 without this, fine in Vibe.
- "NULL" -> "null" — intended.

## require per-document overrides

- Roman Numerals require per-document overrides to work well, maybe even per
  section overrides. Workaround -- roman numerals only for capitalized letters
  for now, only because the ambiguous cases don't come up in our current
  set of demo documents.
  * in `authorship.txt`, (DDD,xxx,bb) could be (XXX, yyy, ff) and I don't think
    in that context it should be read as a roman numeral
  * "Please read the last three sections (IX, XX, XI) of my research paper."
    should be read as roman numerals, but hard to specify detect the difference
    of context.

## D. Multiparty listen-test findings (unresolved)

From listening to `vibe/data/odoru_multiparty_normalized_generated.wav`
(generated before the `--`→space revert and before the `<4b:mi>` /
`<ref-1.l:i;ll>` override updates). Normalized text -> what VibeVoice said:

- `<Ref-8>.)` -> "Ref 8.)" -> "Ref 8 prawnnd"
- `Recorded Mail -- AUGMENT's Journal System.` -> "Recorded Mail ,
  augment's Journal System." -> "Recorded Mail at moinky Nob mc neer
  L'AUGMENT's Journal System"
- `-- e.g.,` -> ", for example" -> "prawned for example"
- `<OAD,2237,>` -> "O A D, two thousand two hundred thirty seven" ->
  "Subtle A DDD, 2237"
- `-- especially` -> ", especially" -> "prawned o especially"
- `(EEE,yy,cc).` -> "(E E E,yy,cc)." -> "EEE, yy, cero sake"
- `and "zzz"` -> "zzz" (unchanged) -> "zizzle zee"

Notes:
- The "prawned"/"prawnnd" hallucinations appear right after every `--`
  in this section — likely caused by the `--`→"," change, which has
  since been reverted back to spaces. Needs re-testing.
- `<OAD,2237,>` and `(EEE,yy,cc)` garbles look unrelated to `--` — may be
  triggered by trailing/leading punctuation (`,>`, `).`) or by spelling
  out large numbers next to letter codes. Not yet diagnosed.
- `"zzz"` -> "zizzle zee" confirms the placeholder-string issue noted
  above: lowercase letter-runs left as-is aren't reliably spelled out by
  VibeVoice.

**Next steps** (pending pod restart):
- Re-normalize `vibe/data/odoru_multiparty.txt` and
  `vibe/data/odoru_traveling.txt` with the `--`→space revert and the
  current `tts_overrides.txt` (`<4b:mi>`, `<ref-1.l:i;ll>`).
- Regenerate audio for both and listen again — check whether the
  "prawned"/"prawnnd" hallucinations are gone.
- If `<OAD,2237,>` and `(EEE,yy,cc)` still garble, investigate trailing
  punctuation / large-number-next-to-code patterns separately.
- The 22 pre-chunked segments in `vibe/data/segments/` (seg01-seg22,
  normalized) are ready but not yet run through VibeVoice — run once the
  above listen tests look good.

## E. `augment_multiparty`/`augment_traveling` listen-test findings
(2026-06-14, two runs: 7:30pm and 10pm, GPU pod `ypl1py60u8knen`)

`augment_traveling`:
- 7:30pm: `"<4b:mi>"` -> "cree-aw" / `"<Ref-1.l:i;LL>"` -> "Ref 1.L
  view i filter LL" (latter is correct)
- 10pm: `"<4b:mi>"` -> "day 4b m eye daya" (still wrong, different
  hallucination each run — non-deterministic)

`augment_multiparty` — overall **improvement** vs. the pre-`--`-revert
run in section D (the "prawned"/"prawnnd" pattern after `--` is gone),
but new/different hallucinations on bracket-heavy strings, and they
differ between runs (non-deterministic):
- `<Ref-8>.)` -> 7:30pm "Ref 8 prawnnd" / 10pm "Ref 8 d a breet"
- `"<OAD,2237,>"` -> 7:30pm "Subtle A DDD, 2237" / 10pm "C O A D, 2237
  day here"
- `(EEE,yy,cc).` -> 7:30pm "EEE, yy, cero sake" / 10pm "EEE, yy, c sef"
- `and "zzz"` -> 7:30pm "zizzle zee" / 10pm "zi zi zizzeh"
- `Recorded Mail -- AUGMENT's Journal System` -> "Recorded Mail at
  moinky Nob mc neer L'AUGMENT's Journal System" (7:30pm; not
  reported for 10pm)

**Conclusion**: hallucinations cluster on tokens with complex trailing
punctuation — `<...>`, `(...)`, `[...]` combined with `.`, `,`, `)`
right at the end. Done: strip `()`, `<>`, `[]` (the bracket characters
themselves, keeping their contents) at the end of normalization, fixed
`apply_punctuated_overrides` to also match keys containing `:` (so
`<4b:mi>` -> `4 b colon M I` now applies), and added `zzz` to
`tts_overrides.txt`.

**Open**: other punctuation that may need similar treatment —
`-` (already mostly handled, becomes space), `;`, and `/` (e.g. "1/2"
as a fraction). Not yet known whether these cause hallucinations;
revisit if found in future listen tests, likely via
`apply_punctuated_overrides` or a dedicated fraction pass.

## F. Re-test after bracket-stripping + colon-override fix
(2026-06-14, pod `kdkms4m3d7opyt`, RTX A4000, GPU)

- `augment_traveling` — **passes**. `<4b:mi>` -> "4 b colon M I" via
  override, `<Ref-1.l:i;LL>` correct as before.
- `augment_multiparty` — much closer, one remaining issue:
  - `"<OAD,2237,>"` -> normalized to "OADD237" (digits/letters run
    together, comma lost) -> still mispronounced. Added override
    `<OAD,2237,>` -> `O A D comma 2 2 3 7 comma` to
    `tts_overrides.txt`. This also surfaced another instance of the
    `apply_punctuated_overrides` matching bug (section E/the `:` fix)
    — the key contains `,` not `.`/`:`, so it was silently skipped.
    Generalized the filter to match any override key with non-
    alphanumeric chars; confirmed via `cargo run -- normalize` that
    `"<OAD,2237,>".` -> `"O A D comma 2 2 3 7 comma".`. Not yet
    re-tested with audio.
  - `<Ref-8>.)` and `(EEE,yy,cc).` and `"zzz"` issues from section E
    appear resolved (not re-reported).

**Next**: re-run `augment_multiparty` with the new `<OAD,2237,>`
override to confirm.

