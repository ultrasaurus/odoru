# Text Normalization

Before synthesis, all text passes through `normalizer.rs`. The goal is to
produce a string that VibeVoice will pronounce correctly and naturally.

Normalization is applied per-sentence, after sentence splitting.

---

## Processing order

- **Pass 0** — Preprocessing
  - a) Punctuated overrides (runs first so keys like `"."` and `"*D"`
       match before quote-stripping removes their surrounding quotes)
  - b) Strip short inline quotes (≤5 words)
- **Pass 1** — Expand `<Tag-N>` markers
- **Pass 2** — Expand year ranges (e.g. `1976-77`)
- **Pass 3** — Replace em-dashes and double-hyphens (` -- ` → `, `)
- **Pass 4** — Expand comma-grouped numbers (`2,000` → `two thousand`)
- **Pass 5** — Spell Item/reference numbers digit-by-digit
  (`Item 71279` → `Item seven one two seven nine`)
- **Pass 6** — Replace identifier dots (`4b.l` → `4b dot l`)
- **Pass 7** — Tokenize and process each token:
  - Override lookup (single-word)
  - Alphanumeric splitting
  - All-caps handling (spell-out or lowercase)
  - Roman numeral expansion
  - Leading-zero digit sequences (`0609` → `zero six zero nine`)
- **Pass 8** — Remaining hyphens → spaces
- **Pass 9** — Strip bracket characters (`<>`, `[]`, `()`)
- **Pass 10** — Strip ellipsis (` ...` and `…`, with leading space)

---

## Rules

### Pass 0a — Overrides file (punctuated)

`tts_overrides.txt` is loaded at startup from next to the binary, falling back
to the current working directory. Edits take effect on the next run — no
recompile needed.

Format: two whitespace-separated columns. Lines starting with `# ` (hash
space) are comments; lines starting with `#` followed by a non-space are
valid override keys (e.g. `#x`, `#s`).

Keys containing any non-alphanumeric character are applied as full-text
find-and-replace before tokenization. Single-word (alphanumeric-only) keys
are matched during tokenization (Pass 7).

Match is **case-insensitive**. Replacement is used **exactly as written**.

---

### Pass 0b — Strip short inline quotes

Quoted phrases of 5 words or fewer have their surrounding quotation marks
removed. This prevents TTS mangling of brief quoted phrases.

| Input | Output |
|-------|--------|
| `the "OK Key" action` | `the OK Key action` |
| `called "viewspecs."` | `called viewspecs.` |

Longer quoted passages are left intact.

---

### Pass 1 — Tag expansion

Citation, figure, and table markers are expanded to spoken form.

| Input | Output |
|-------|--------|
| `<Ref-3>` | `Ref 3` |
| `<Fig-1>` | `Figure 1` |
| `<Table-2>` | `Table 2` |
| `<Foo-3>` | `Foo 3` *(fallback: any `<CapWord-N>`)* |

Tags where the word part is lowercase (e.g. `<em>`, `<b>`) are left
untouched to avoid mangling HTML.

---

### Pass 2 — Year ranges

Any hyphen between two runs of digits is expanded to "to".

| Input | Output |
|-------|--------|
| `1976-77` | `1976 to 77` |
| `pages 10-20` | `pages 10 to 20` |

---

### Pass 3 — Em-dash / double-hyphen

` -- ` and `—` are replaced with `, ` to give the model a clean pause cue
instead of extra whitespace that causes vocalization artifacts.

| Input | Output |
|-------|--------|
| `Statement 3c -- and` | `Statement 3c, and` |
| `lines — things` | `lines, things` |

---

### Pass 4 — Comma-grouped numbers

Numbers with comma separators are expanded to words.

| Input | Output |
|-------|--------|
| `2,000` | `two thousand` |
| `100,000` | `one hundred thousand` |

---

### Pass 5 — Item/reference numbers

Multi-digit numbers (4+ digits) following `Item` or `Ref` are spelled out
digit-by-digit so TTS doesn't garble large IDs.

| Input | Output |
|-------|--------|
| `Item 71279` | `Item seven one two seven nine` |
| `Ref 14724` | `Ref one four seven two four` |

Numbers with commas (e.g. `2,345`) are excluded — handled by Pass 4.

---

### Pass 6 — Identifier dots

A `.` followed by an alphanumeric character, not preceded by another `.`,
is replaced with ` dot `. Catches link notation like `4b.l` and `.dt`.

| Input | Output |
|-------|--------|
| `4b.l` | `4b dot l` |
| `Ref.dt` | `Ref dot dt` |
| `.l` | `dot l` |

---

### Pass 7 — Tokenization

The text is split at non-alphanumeric boundaries. Each token is processed:

**a. Override lookup** — single-word match in `tts_overrides.txt`
(case-insensitive); replacement used as-is, no further rules apply.

**b. Alphanumeric splitting** — tokens mixing digits and letters have spaces
inserted between runs.

| Input | Output |
|-------|--------|
| `4b` | `4 b` |
| `4c2` | `4 c 2` |

**c. All-caps, short (≤3 letters, not a Roman numeral)** — spelled out
letter by letter.

| Input | Output |
|-------|--------|
| `UIS` | `U I S` |
| `TTS` | `T T S` |

**d. Roman numerals** — uppercase (any length) and lowercase (up to C=100)
expanded to words.

| Input | Output |
|-------|--------|
| `VIII` | `eight` |
| `iv` | `four` |

**e. All-caps, long (>3 letters)** — lowercased so the model reads them as
normal words.

| Input | Output |
|-------|--------|
| `AUGMENT` | `augment` |
| `AUGMENT's` | `augment's` |

**f. Leading-zero sequences** — tokens starting with `0` and containing
only digits are spelled out digit-by-digit.

| Input | Output |
|-------|--------|
| `0609` | `zero six zero nine` |
| `069` | `zero six nine` |

---

### Pass 8 — Hyphens → spaces

Remaining hyphens are replaced with spaces.

| Input | Output |
|-------|--------|
| `bit-mapped` | `bit mapped` |
| `on-line` | `on line` |

---

### Pass 9 — Strip brackets

Bracket characters `<`, `>`, `[`, `]`, `(`, `)` are removed (contents
kept). Prevents hallucinations on complex trailing punctuation.

---

### Pass 10 — Strip ellipsis

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
- **Bare numbers** — digits passed through as-is. Generally handled well
  in context; add to `tts_overrides.txt` if needed.
