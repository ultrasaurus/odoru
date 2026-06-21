# Text Normalization

Before synthesis, all text passes through `normalizer.rs`. The goal is to
produce a string that the TTS backend (Kokoro or F5/VibeVoice) will
pronounce correctly and naturally — and, since alignment models only have
letters in their vocabulary, a string with no bare digit characters left
for forced alignment to silently drop.

Normalization is applied per-sentence, after sentence splitting. Pass
numbers below match the `// Pass N:` comments in `normalize_with_spans`
(`util/src/normalizer.rs`) exactly — if they drift, fix the code comments
first, since they're the source of truth.

---

## Processing order

- **Pass 1** — Punctuated overrides (runs first, before quote-stripping, so
  keys like `"."` and `"*D"` can match before their surrounding quotes are
  removed)
- **Pass 2** — Strip short inline quotes (≤5 words)
- **Pass 3** — Expand `<Tag-N>` markers
- **Pass 4** — Expand year ranges (e.g. `1976-77`)
- **Pass 5** — Replace em-dashes and double-hyphens (` -- ` → `, `)
- **Pass 6** — Spell Item/reference numbers digit-by-digit
  (`Item 71279` → `Item seven one two seven nine`) — runs before Pass 7 so
  Item/Ref numbers keep their ID-style digit-by-digit reading rather than
  being shadowed by the generic bare-number rule
- **Pass 7** — Expand bare numbers not already handled above: comma-grouped
  (`2,000` → `two thousand`) or digit-group style for bare 1-4 digit runs
  (`560` → `five sixty`, `1976` → `nineteen seventy six`)
- **Pass 8** — Expand journal links (`(AUGMENT,71279,)` → `Augment seven
  one two seven nine`)
- **Pass 9** — Expand US state postal abbreviations in `"City, ST"`
  position (`Denver, CO` → `Denver, Colorado`)
- **Pass 10** — Replace identifier dots (`4b.l` → `4b dot l`)
- **Pass 11** — Tokenize and process each token:
  - Override lookup (single-word)
  - Alphanumeric splitting
  - Roman numeral expansion (all-caps stems, 2+ letters)
  - All-caps handling (letter-spell ≤3 letters, title-case longer)
  - Leading-zero digit sequences (`0609` → `zero six zero nine`)
  - Remaining hyphens → spaces
- **Pass 12** — Strip bracket characters (`<>`, `[]`, `()`)
- **Pass 13** — Strip ellipsis (` ...` and `…`, with leading space)

---

## Rules

### Pass 1 — Overrides file (punctuated)

`tts_overrides.txt` is loaded at startup from next to the binary, falling back
to the current working directory. Edits take effect on the next run — no
recompile needed.

Format: two tab-separated columns. Using a tab (not spaces) as the
delimiter allows keys to contain spaces (e.g. multi-word punctuated
phrases like `I. INTRODUCTION`). Lines starting with `# ` (hash space)
are comments; lines starting with `#` followed by a non-space are valid
override keys (e.g. `#x`, `#s`).

Keys containing any non-alphanumeric character are applied as full-text
find-and-replace before tokenization. Single-word (alphanumeric-only) keys
are matched during tokenization (Pass 11).

Match is **case-insensitive**. Replacement is used **exactly as written**.

---

### Pass 2 — Strip short inline quotes

Quoted phrases of 5 words or fewer have their surrounding quotation marks
removed. This prevents TTS mangling of brief quoted phrases.

| Input | Output |
|-------|--------|
| `the "OK Key" action` | `the OK Key action` |
| `called "viewspecs."` | `called viewspecs.` |

Longer quoted passages are left intact.

---

### Pass 3 — Tag expansion

Citation, figure, and table markers are expanded to spoken form.

| Input | Output |
|-------|--------|
| `<Ref-3>` | `Ref 3` |
| `<Fig-1>` | `Figure 1` |
| `<Table-2>` | `Table 2` |
| `<Foo-3>` | `Foo 3` *(fallback: any `<CapWord-N>`)* |

Tags where the word part is lowercase (e.g. `<em>`, `<b>`) are left
untouched to avoid mangling HTML. The trailing number is left as a bare
digit — see the per-chunk-granularity note in Implementation below for why
Pass 7 doesn't also spell it out.

---

### Pass 4 — Year ranges

Any hyphen between two runs of digits is expanded to "to".

| Input | Output |
|-------|--------|
| `1976-77` | `1976 to 77` |
| `pages 10-20` | `pages 10 to 20` |

The digit runs themselves are left bare here too, for the same reason as
Pass 3 — see Implementation.

---

### Pass 5 — Em-dash / double-hyphen

` -- ` and `—` are replaced with `, ` to give the model a clean pause cue
instead of extra whitespace that causes vocalization artifacts.

| Input | Output |
|-------|--------|
| `Statement 3c -- and` | `Statement 3c, and` |
| `lines — things` | `lines, things` |

---

### Pass 6 — Item/reference numbers

Multi-digit numbers (4+ digits) following `Item` or `Ref` are spelled out
digit-by-digit so TTS doesn't garble large IDs — these read as identifiers,
not magnitudes.

| Input | Output |
|-------|--------|
| `Item 71279` | `Item seven one two seven nine` |
| `Ref 14724` | `Ref one four seven two four` |
| `Item 1000` | `Item one zero zero zero` |
| `Item 42` | `Item 42` *(fewer than 4 digits — left to Pass 7)* |

Numbers with commas (e.g. `Item 2,345`) are excluded — Pass 6 only counts
contiguous digit runs, so a comma-grouped number never reaches the 4-digit
threshold here and falls through to Pass 7 instead.

---

### Pass 7 — Bare number expansion

Any number not already handled by Pass 6 gets spelled out, so TTS reads it
clearly and forced alignment (vocabulary is letters only) can time-align
it — a bare digit string is otherwise silently dropped by alignment.

**Comma-grouped** numbers expand via `number_to_words` (the `num2words`
crate):

| Input | Output |
|-------|--------|
| `2,000` | `two thousand` |
| `100,000` | `one hundred thousand` |

**Bare 1-2 digit** numbers spell out directly:

| Input | Output |
|-------|--------|
| `5` | `five` |
| `0` | `zero` |
| `42` | `forty two` |

**Bare 3-digit** numbers read digit-group style (hundreds digit alone, then
the remaining two digits as a unit):

| Input | Output |
|-------|--------|
| `560` | `five sixty` |
| `501` | `five oh one` *(leading zero in the last two digits)* |
| `500` | `five hundred` *(round)* |

**Bare 4-digit** numbers read as two 2-digit groups, the way years and
addresses are normally spoken:

| Input | Output |
|-------|--------|
| `1976` | `nineteen seventy six` |
| `2010` | `twenty ten` |
| `2005` | `twenty oh five` *(leading zero in 2nd group)* |
| `1900` | `nineteen hundred` *(round hundred)* |
| `2000` | `two thousand` *(round thousand — `first group ends in 0`, reads as a single cardinal instead of "twenty hundred")* |

**Excluded** (left bare, handled elsewhere or not yet handled):
- Leading-zero runs of length > 1 (`05`, `0609`) — IDs, spelled
  digit-by-digit in Pass 11 instead.
- A digit run touching a letter on either side (`4b`, `14B`, `v2`) — an
  alphanumeric ID, not a bare number; `split_alphanumeric` (Pass 11) spaces
  these out instead.
- 5+ digit bare numbers — not yet handled; still read as raw digits.

---

### Pass 8 — Journal links

Patterns like `(AUGMENT,71279,)` or `<OAD,2237,>` — an optional bracket,
2+ uppercase letters, comma, digits, comma, optional closing bracket — are
expanded: the name is title-cased, the digits spelled digit-by-digit.

| Input | Output |
|-------|--------|
| `(AUGMENT,71279,)` | `Augment seven one two seven nine` |
| `<OAD,2237,>` | `O A D two two three seven` |
| `(DDD,xxx,bb)` | `(DDD,xxx,bb)` *(lowercase name — not matched; these are placeholder strings, not real links)* |

---

### Pass 9 — US state abbreviations

Expands a 2-letter state postal code only in the `"City, ST"` position
(comma immediately before it) — not as a standalone-word override, since
many codes (`IN`, `OR`, `ME`, `HI`, `OK`, `OH`, ...) collide with common
English words; a flat word-list override would mangle those everywhere
they appear.

| Input | Output |
|-------|--------|
| `Denver, CO` | `Denver, Colorado` |
| `in Colorado` | `in Colorado` *(no comma — "CO" wouldn't even appear lowercase here)* |

---

### Pass 10 — Identifier dots

A `.` followed by an alphanumeric character, not preceded by another `.`,
is replaced with ` dot `. Catches link notation like `4b.l` and `.dt`.

| Input | Output |
|-------|--------|
| `4b.l` | `4b dot l` |
| `Ref.dt` | `Ref dot dt` |
| `.l` | `dot l` |

---

### Pass 11 — Tokenization

The text is split at non-alphanumeric boundaries. Each token is processed:

**a. Override lookup** — single-word match in `tts_overrides.txt`
(case-insensitive); replacement used as-is, no further rules apply.

**b. Alphanumeric splitting** — tokens mixing digits and letters have spaces
inserted between runs.

| Input | Output |
|-------|--------|
| `4b` | `4 b` |
| `4c2` | `4 c 2` |

**c. Roman numerals** — all-caps stems of 2+ letters are checked first;
lowercase Roman numerals are intentionally not handled (see Known
limitations).

| Input | Output |
|-------|--------|
| `VIII` | `eight` |
| `IV` | `four` |

**d. All-caps, short (≤3 letters, not a Roman numeral)** — spelled out
letter by letter.

| Input | Output |
|-------|--------|
| `UIS` | `U I S` |
| `TTS` | `T T S` |

**e. All-caps, long (>3 letters)** — title-cased (first letter capitalized,
rest lowercased) so the model reads them as a normal word rather than
letter-spelling them.

| Input | Output |
|-------|--------|
| `AUGMENT` | `Augment` |
| `AUGMENT's` | `Augment's` |

**f. Leading-zero sequences** — tokens starting with `0`, length > 1,
containing only digits, are spelled out digit-by-digit.

| Input | Output |
|-------|--------|
| `0609` | `zero six zero nine` |
| `069` | `zero six nine` |

**g. Remaining hyphens** — replaced with spaces.

| Input | Output |
|-------|--------|
| `bit-mapped` | `bit mapped` |
| `on-line` | `on line` |

---

### Pass 12 — Strip brackets

Bracket characters `<`, `>`, `[`, `]`, `(`, `)` are removed (contents
kept). Prevents hallucinations on complex trailing punctuation.

---

### Pass 13 — Strip ellipsis

` ...`, `...`, ` …`, and `…` are removed. Leading space is stripped with
the ellipsis to avoid orphaned punctuation (e.g. `every ...,` → `every,`
not `every ,`).

---

## Known limitations

- **Context-dependent abbreviations** — `Dr.` always expands to `Doctor`.
  Drive vs. Doctor is not handled.
- **Fractions** — `1/2` becomes `1 2` (slash → space). No fraction
  expansion yet. Use `tts_overrides.txt` for specific cases.
- **Uneven quote stripping** — `strip_short_quotes` removes quotes around
  ≤5-word phrases but does not track open/close balance; mismatched quotes
  can result in malformed text (e.g. seg20 CLI command strings).
- **5+ digit bare numbers** — still passed through as raw digits (Pass 7
  only covers 1-4 digit runs). Add to `tts_overrides.txt` if a specific
  case needs fixing.
- **Lowercase Roman numerals** — not expanded; see Pass 11c. Disambiguating
  "xiv" (= 14) from a lowercase placeholder string (e.g. "xxx" in
  `(DDD,xxx,bb)`) isn't reliably possible without per-document context.

---

# Implementation

## Why span-mapping, not diffing

Forced alignment (used for the annotation "click to listen" feature, see
[annotation.md](annotation.md)) needs to map word timestamps — which land
on *normalized* text — back to character offsets in the *original*
sentence text that annotations are anchored to.

The normalizer produces this mapping as a byproduct of normalization
itself, rather than reconstructing it after the fact by diffing normalized
text against the original. Diffing is fragile: it silently breaks whenever
a normalization rule changes the shape of its output (e.g. adding a new
case, reordering passes), since the diff algorithm has no notion of which
text came from which transformation. A forward mapping — built by the same
code that does the transforming — stays correct by construction; if a pass
is wrong, its mapping is wrong too, but at least consistently and visibly
wrong rather than silently misaligned.

## The `Spanned` chunk model

`normalize_with_spans` threads a `Vec<Spanned>` through all 13 passes,
where each `Spanned { text: String, src: Range<usize> }` is a chunk of
*normalized* text tagged with the char range in the *original* input it
derives from. `src` always anchors to the **original** input, never an
intermediate pass's output — that's what lets spans compose correctly
across multiple passes without each pass needing to know about any other.

`is_raw(chunk)` — `chunk.text.chars().count() == chunk.src.len()` — is true
only if no earlier pass has expanded or contracted the chunk's text. Every
scan-and-subdivide pass checks this and returns a non-raw chunk unchanged
rather than re-scanning into it, since local char offsets stop
corresponding to source offsets once a chunk has been expanded.

`NormalizedText::source_range(normalized_range)` is the public lookup: give
it a char range in the final normalized text, get back the corresponding
range in the original input (or `None` if out of bounds).

### Granularity is per-chunk, not per-word

A chunk that gets expanded (e.g. `<Ref-3>` → `Ref 3`, or `Item 71279` →
`Item seven one two seven nine`) maps as a **whole** back to its source
range — there's no attempt to map individual output words within an
expansion back to individual input characters. This is why Pass 3's
`<Ref-3>` → `Ref 3` doesn't get its `3` further spelled out by Pass 7 even
though it's a bare digit at that point: the whole `Ref 3` is one non-raw
chunk, and Pass 7's bare-number scan correctly skips into it.

It's also why, when mapping forced-alignment words back to original text
(`tts::alignment::words_with_original_text`), multiple aligned words that
land in the same expanded chunk (e.g. all six of "Item"/"seven"/"one"/
"two"/"seven"/"nine") get merged into one output entry rather than
returned as repeated duplicates — returning duplicates would break a
client doing `indexOf` on the joined text, since it'd match the first,
too-early occurrence.

## Three bug categories found while converting passes

Converting all 13 passes from flat-string transforms to chunk-aware ones
surfaced three recurring correctness traps, worth knowing before adding a
14th pass:

1. **Stale local-offset math on non-raw chunks.** A scan-and-subdivide pass
   that doesn't check `is_raw` first will recompute `base + local_offset`
   against a chunk whose local offsets no longer match source offsets,
   producing wrong spans. Fix: always check `is_raw` and return non-raw
   chunks unchanged.

2. **Naive length-changing "uniform map" passes.** A pass that changes a
   chunk's text length without subdividing it (e.g. an early em-dash → `, `
   implementation, or bracket-stripping done as a single string replace)
   silently breaks the `is_raw` invariant for the **whole** chunk,
   including unrelated trailing text — hiding that trailing text from every
   later pass. Fixed via a generic `replace_literal_spanned` /
   `replace_literal_chunk` helper that does proper flush-then-subdivide
   instead of a flat string replace.

3. **Patterns that can legitimately span chunk boundaries.** `Pass 2`
   (`strip_short_quotes`) needs to find quote pairs that can end up split
   across multiple chunks, because Pass 1 (`apply_punctuated_overrides`)
   runs first and can split text mid-quote-pair via an override match. A
   per-chunk-independent scan can't see across that split. This is the one
   pass in the whole pipeline that needs a global scan instead: flatten
   once, find removal positions in flattened-text coordinates, then
   surgically delete just those positions from whichever original chunks
   they land in. Documented here as a known general limitation — if a
   future pass needs to match a pattern that an earlier pass might split
   across chunks, it'll need the same approach, not a per-chunk scan.

## Pipeline ordering matters more than it looks

Pass 6 (Item/Ref digit-by-digit) must run before Pass 7 (generic bare
numbers) — not just any order: the more *specific* rule has to claim its
match and mark the chunk non-raw before the more *generic* rule's scan
would otherwise have reached the same text. Reordering these the other way
silently breaks Pass 6's ID-style reading (e.g. "Item 1000" → "Item one
thousand" instead of the intended "Item one zero zero zero") with no
compiler error — only a behavioral regression, caught by tests. When
adding a new pass whose pattern overlaps an existing one, place the more
specific pass first.
