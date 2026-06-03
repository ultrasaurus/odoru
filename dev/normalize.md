# Text Normalization

Before synthesis, all text passes through `normalizer.rs`. The goal is to
produce a string that F5-TTS will pronounce correctly and naturally.

Normalization is applied per-sentence, after sentence splitting.

---

## Processing order

1. Expand `<Tag-N>` markers
2. Expand numeric ranges
3. Apply punctuated overrides from `tts_overrides.txt`
4. Tokenize and process each token:
   - Override lookup
   - Alphanumeric splitting
   - All-caps handling (spell-out or lowercase)
5. Hyphenated words (hyphens → spaces)

---

## Rules

### 1. Tag expansion

Citation, figure, and table markers are expanded to spoken form.

| Input | Output |
|-------|--------|
| `<Ref-3>` | `Ref 3` |
| `<Ref-12>` | `Ref 12` |
| `<Fig-1>` | `Figure 1` |
| `<Table-2>` | `Table 2` |
| `<Foo-3>` | `Foo 3` *(fallback: any `<CapWord-N>`)* |

Tags where the word part is lowercase (e.g. `<em>`, `<b>`) are left untouched
to avoid mangling HTML.

---

### 2. Numeric ranges

Any hyphen between two runs of digits is expanded to "to".

| Input | Output |
|-------|--------|
| `1976-77` | `1976 to 77` |
| `1976-1977` | `1976 to 1977` |
| `pages 10-20` | `pages 10 to 20` |

---

### 3. Overrides file

`tts_overrides.txt` is loaded at startup from next to the binary, falling back
to the current working directory. Edits take effect on the next run — no
recompile needed.

Format: two whitespace-separated columns, `#` for comments.

```
# match       replacement
e.g.          for example
i.e.          that is
etc           et cetera
Dr            Doctor
HTTP          HTTP        # preserve as spoken word, not H T T P
PIN           P I N       # spell out, not "pin"
```

- Match is **case-insensitive**.
- The replacement is used **exactly as written** (case preserved).
- Entries containing `.` are applied as a full-text find-and-replace before
  tokenization, so they can match across punctuation (e.g. `e.g.`).
- Single-word entries are matched during tokenization.

---

### 4. Tokenization

The text is split into tokens at non-alphanumeric boundaries (spaces,
punctuation, etc.). Each token is then processed in order:

**a. Override lookup** — if the token matches an entry in `tts_overrides.txt`
(case-insensitive), the replacement is used as-is and no further rules apply.

**b. Alphanumeric splitting** — tokens mixing digits and letters have spaces
inserted between runs, so section labels are read character by character.

| Input | Output |
|-------|--------|
| `1a` | `1 a` |
| `4c2` | `4 c 2` |
| `4C2` | `4 C 2` |
| `12ab34` | `12 ab 34` |

**c. All-caps, short (≤ 3 letters)** — spelled out letter by letter. F5-TTS
tends to mispronounce or clip short acronyms when presented as a unit.

| Input | Output |
|-------|--------|
| `UIS` | `U I S` |
| `TTS` | `T T S` |
| `DNA` | `D N A` |

To override: add to `tts_overrides.txt`. For example, to keep `PIN` as a
spelled-out word rather than "pin": `PIN    P I N`. To keep `HTTP` as a word
rather than `H T T P`: `HTTP    HTTP`.

**d. All-caps, long (> 3 letters)** — lowercased so F5-TTS reads them as
normal words.

| Input | Output |
|-------|--------|
| `AUGMENT` | `augment` |
| `IMPORTANT` | `important` |
| `AUGMENT's` | `augment's` |

---

### 5. Hyphenated words (hyphens → spaces)

Hyphens between words are replaced with spaces. F5-TTS introduces an
unnatural pause at hyphens that is longer than a natural word boundary.

| Input | Output |
|-------|--------|
| `one-handed` | `one handed` |
| `on-line` | `on line` |

Numeric range hyphens (rule 2) and tag hyphens (rule 1) are handled in earlier
passes and are not affected by this rule.

---

## Known limitations

- **Context-dependent abbreviations** — `Dr.` always expands to `Doctor`.
  The distinction between *Doctor* and *Drive* is not handled.
- **Occasional model mispronunciation** — some word combinations are
  mispronounced by F5-TTS regardless of normalization (e.g. `author class`
  vs `author workshop`). This is a model artifact and cannot be fixed via
  text preprocessing.
- **Bare numbers** — digits passed through as-is. F5-TTS generally handles
  these well in context but may stumble in unusual surroundings. Add specific
  cases to `tts_overrides.txt` if needed.
