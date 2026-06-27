/// normalizer.rs — text normalization applied before TTS synthesis.
///
/// Processing order:
///   0. Strip short inline quotes (≤5 words): "foo bar" → foo bar.
///   1. Expand known `<Tag-N>` citation/figure/table markers.
///   2. Expand year ranges: 1976-77 → "1976 to 77".
///   2b. Expand comma-grouped numbers: 2,000 → "two thousand".
///   2c. Spell item/reference numbers digit-by-digit: Item 71279 → Item seven one …
///   2d. Expand journal links: (AUGMENT,71279,) → Augment seven one two seven nine
///   3. Load `tts_overrides.txt` and apply punctuated overrides (e.g. "e.g.").
///   3b. Replace dots between alphanumeric chars with " dot ": 4b.l → 4b dot l.
///   4. Tokenize on word boundaries:
///      a. Apply single-word overrides (case-insensitive).
///      b. Spell out short all-caps (≤3 chars) letter by letter: UIS → U I S.
///      c. Lowercase long all-caps (>3 chars): AUGMENT → augment.
///      d. Insert spaces in alphanumeric tokens: 1a → 1 a, 4c2 → 4 c 2.
///      e. Spell leading-zero digit strings word-by-word: 0609 → zero six zero nine.
///   5. Replace remaining hyphens with spaces.
///   6. Strip bracket characters.
///   7. Replace ellipses with newlines (with punctuation normalisation).
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};
use tracing::error;

// ---------------------------------------------------------------------------
// Override table
// ---------------------------------------------------------------------------

struct Overrides {
    map: Arc<RwLock<HashMap<String, String>>>,
    /// Path to `tts_overrides.txt` that was loaded, and will be written back to.
    path: PathBuf,
}

static OVERRIDES: OnceLock<Overrides> = OnceLock::new();

fn find_overrides_path() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    let exe_path = exe.parent().unwrap_or(std::path::Path::new("."))
        .join("tts_overrides.txt");
    let workspace_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tts_overrides.txt");
    let cwd_path = PathBuf::from("tts_overrides.txt");

    [exe_path, workspace_path, cwd_path]
        .into_iter()
        .find(|p| p.exists())
        .unwrap_or_default()
}

fn parse_overrides(contents: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in contents.lines() {
        let line = line.trim();
        // "# " (hash + space) starts a comment; bare "#x" etc. are valid keys.
        if line.is_empty() || line.starts_with("# ") { continue; }
        // Tab is the required delimiter so that keys can contain spaces
        // (e.g. multi-word punctuated keys like "I. INTRODUCTION").
        let mut cols = line.splitn(2, '\t');
        match (cols.next(), cols.next()) {
            (Some(from), Some(to)) => {
                let to = to.trim().split('#').next().unwrap_or("").trim();
                if !to.is_empty() {
                    map.insert(from.to_lowercase(), to.to_owned());
                }
            }
            (Some(from), None) => {
                tracing::warn!("tts_overrides.txt: no tab on line {:?} — entry ignored (use tab to separate key from value)", from);
            }
            _ => {}
        }
    }
    map
}

fn init_overrides() -> Overrides {
    let path = find_overrides_path();
    let map = std::fs::read_to_string(&path)
        .map(|s| parse_overrides(&s))
        .unwrap_or_default();
    Overrides { map: Arc::new(RwLock::new(map)), path }
}

fn state() -> &'static Overrides {
    OVERRIDES.get_or_init(init_overrides)
}

fn read_map() -> std::sync::RwLockReadGuard<'static, HashMap<String, String>> {
    state().map.read().expect("overrides lock poisoned")
}

/// Save `map` to `path` as tab-separated lines, sorted by key.
fn save_map_in(path: &std::path::Path, map: &HashMap<String, String>) {
    let mut lines: Vec<String> = map.iter()
        .map(|(k, v)| format!("{k}\t{v}"))
        .collect();
    lines.sort();
    let contents = lines.join("\n") + "\n";
    if let Err(e) = std::fs::write(path, &contents) {
        error!("failed to write tts_overrides.txt: {e}");
    }
}

fn save_map(map: &HashMap<String, String>) {
    save_map_in(&state().path, map);
}

// ---------------------------------------------------------------------------
// Public override management API
// ---------------------------------------------------------------------------

/// Insert/update `word` -> `replacement` (lowercased key) in `map`. Pure —
/// no I/O — so it can be unit tested without touching the global override
/// table or `tts_overrides.txt`.
fn add_override_in(map: &mut HashMap<String, String>, word: &str, replacement: &str) {
    map.insert(word.to_lowercase(), replacement.to_owned());
}

/// Remove `word` (lowercased key) from `map`. Returns true if it existed.
fn remove_override_in(map: &mut HashMap<String, String>, word: &str) -> bool {
    map.remove(&word.to_lowercase()).is_some()
}

/// Sorted (word, replacement) pairs from `map`.
fn list_overrides_in(map: &HashMap<String, String>) -> Vec<(String, String)> {
    let mut pairs: Vec<_> = map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    pairs
}

/// Add or update a pronunciation override and persist to disk.
pub fn add_override(word: &str, replacement: &str) {
    let mut map = state().map.write().expect("overrides lock poisoned");
    add_override_in(&mut map, word, replacement);
    save_map(&map);
}

/// Remove a pronunciation override and persist to disk. Returns true if it existed.
pub fn remove_override(word: &str) -> bool {
    let mut map = state().map.write().expect("overrides lock poisoned");
    let existed = remove_override_in(&mut map, word);
    if existed { save_map(&map); }
    existed
}

/// Return all current overrides as a sorted vec of (word, replacement) pairs.
pub fn list_overrides() -> Vec<(String, String)> {
    list_overrides_in(&read_map())
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Normalize `text` for TTS pronunciation, discarding the source-span
/// mapping. Most callers want this; use `normalize_with_spans` when you
/// need to map a position in the normalized output back to the original
/// input (e.g. translating forced-alignment word timestamps).
pub fn normalize(text: &str) -> String {
    normalize_with_spans(text).text
}

/// Normalized text plus a mapping from char ranges in `text` back to char
/// ranges in the original (pre-normalization) input.
pub struct NormalizedText {
    pub text: String,
    spans: Vec<Spanned>,
}

impl NormalizedText {
    /// Map a char range in the normalized `text` back to the char range in
    /// the original input it was derived from. Returns `None` if
    /// `normalized_range` is out of bounds.
    ///
    /// Granularity is per-chunk, not per-char (see the span-mapping
    /// proof-of-concept notes above `Spanned`): if `normalized_range` spans
    /// multiple chunks, the result covers the union of their source
    /// ranges, not a precise char-for-char mapping.
    pub fn source_range(&self, normalized_range: std::ops::Range<usize>) -> Option<std::ops::Range<usize>> {
        if normalized_range.start >= normalized_range.end { return None; }
        let start = source_span_at(&self.spans, normalized_range.start)?;
        let last_offset = normalized_range.end - 1;
        let end = source_span_at(&self.spans, last_offset)?;
        Some(start.start.min(end.start)..start.end.max(end.end))
    }
}

/// Same normalization as `normalize`, but keeps the source-span mapping
/// needed to translate positions in the output back to the original input.
///
/// Every pass below runs as a `*_spanned` chunk-threading variant instead
/// of a flat-string transform — this is the single source of truth for
/// `normalize()`'s behavior; the individual unspanned pass functions
/// (`expand_year_ranges`, `tokenize`, etc., used directly by unit tests)
/// are now thin wrappers delegating to these same spanned passes, so there
/// is no separate parallel implementation to drift out of sync.
pub fn normalize_with_spans(text: &str) -> NormalizedText {
    let overrides = read_map();

    let chunks = spanned_from_input(text);

    // Pass 1: punctuated overrides run first — before quote-stripping — so
    // keys like `"."` and `"*D"` can match before their quotes are removed.
    let chunks = apply_punctuated_overrides_spanned(chunks, &overrides);

    // Pass 2: strip short inline quotes (≤5 words) — prevents TTS mangling
    // of brief quoted phrases by removing the quotation marks.
    let chunks = strip_short_quotes_spanned(chunks);

    // Pass 3: expand <Tag-N> markers.
    let chunks = expand_tags_spanned(chunks);

    // Pass 4: expand year ranges (4-digit year, hyphen, 2+ digits).
    let chunks = expand_year_ranges_spanned(chunks);

    // Pass 5: replace em-dashes and double-hyphens with comma so TTS gets a
    // clean pause cue instead of extra whitespace that causes vocalisation artifacts.
    let chunks = replace_em_dash_and_double_hyphen_spanned(chunks);

    // Pass 6: spell Item/reference numbers digit-by-digit (Item 71279 →
    // Item seven one two seven nine) so TTS doesn't garble large IDs. Runs
    // before the generic bare-number rule (Pass 7) — Item/Ref numbers are
    // an ID-style exception (digit-by-digit) that the generic digit-group
    // rule would otherwise shadow for 1-4 digit Item/Ref numbers (e.g.
    // "Item 1000" must stay "Item one zero zero zero", not "Item one
    // thousand"). Once this pass expands a match, the chunk becomes
    // non-raw, so Pass 7 correctly skips re-processing it.
    let chunks = spell_item_numbers_spanned(chunks);

    // Pass 7: expand bare numbers not already handled above — comma-grouped
    // ("2,000" -> "two thousand") or digit-group style for 1-4 digit runs
    // ("560" -> "five sixty", "1976" -> "nineteen seventy six").
    let chunks = expand_comma_numbers_spanned(chunks);

    // Pass 8: expand journal links like (AUGMENT,71279,) or <OAD,2237,> →
    // "Augment 7 1 2 7 9". Runs before bracket-stripping so the delimiters
    // are consumed as part of the pattern.
    let chunks = expand_journal_links_spanned(chunks);

    // Pass 9: expand US state postal abbreviations in "City, ST" position
    // (e.g. "Denver, CO" → "Denver, Colorado"). Only fires on the comma-led
    // pattern, not as a standalone word override, because many codes (IN, OR,
    // ME, HI, OK, OH, ...) collide with common English words — a flat
    // word-list override would mangle those everywhere they appear.
    let chunks = expand_state_abbrevs_spanned(chunks);

    // Pass 10: replace dots between alphanumeric chars with " dot " so link
    // notation like `4b.l` or `Ref.dt` is read correctly.
    let chunks = replace_identifier_dots_spanned(chunks);

    // Pass 11: tokenize and process word/alphanumeric tokens (override
    // lookup, alphanumeric splitting, all-caps handling, Roman numerals,
    // leading-zero digit sequences); replace remaining hyphens with spaces.
    let chunks = tokenize_spanned(chunks, &overrides);

    // Pass 12: strip bracket characters (keeping their contents) — VibeVoice
    // hallucinates on tokens with `<>`/`()`/`[]` next to other punctuation.
    let chunks = strip_brackets_spanned(chunks);

    // Pass 13: replace ellipses with newlines so VibeVoice treats the pause
    // as a sentence boundary rather than looping on an unfinished sentence.
    let chunks = replace_ellipsis_spanned(chunks);

    let text = flatten(&chunks);
    NormalizedText { text, spans: chunks }
}

// ---------------------------------------------------------------------------
// Pass 3: <Tag-N> expansion
// ---------------------------------------------------------------------------

#[cfg(test)]
fn expand_tags(text: &str) -> String {
    flatten(&expand_tags_spanned(spanned_from_input(text)))
}

fn try_expand_tag(inner: &str) -> Option<String> {
    let (word, digits) = inner.split_once('-')?;
    if !word.chars().next()?.is_uppercase() { return None; }
    if !digits.chars().all(|c| c.is_ascii_digit()) { return None; }

    let spoken = match word.to_lowercase().as_str() {
        "fig"   => "figure".to_owned(),
        "table" => "table".to_owned(),
        other   => other.to_owned(),
    };
    let spoken = {
        let mut c = spoken.chars();
        match c.next() {
            None => String::new(),
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        }
    };
    // Spell the digits out now, the same way Pass 4 (year ranges) does —
    // once this tag is expanded the chunk is no longer raw, so Pass 7's
    // bare-number scan can never reach these digits afterward (chunk-
    // granularity design). Left bare, they'd be invisible to forced
    // alignment entirely, not just unspoken awkwardly. Falls back to the
    // bare digits for 5+ digit tags, which `spell_bare_digit_group` doesn't
    // cover.
    let spoken_digits = spell_bare_digit_group(digits).unwrap_or_else(|| digits.to_owned());
    Some(format!("{} {}", spoken, spoken_digits))
}

/// Span-aware version of `expand_tags`. When a `<...>` doesn't expand (no
/// closing `>`, or `try_expand_tag` rejects it), the re-emitted text is
/// identical to the input for that range, so it's simply left in the
/// pending unprocessed run rather than flushed as a no-op replacement.
fn expand_tags_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| expand_tags_chunk(&chunk)).collect()
}

fn expand_tags_chunk(chunk: &Spanned) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    while i < chars.len() {
        if chars[i] == '<' {
            let start = i;
            let mut j = i + 1;
            while j < chars.len() && chars[j] != '>' && chars[j] != '<' { j += 1; }
            if j < chars.len() && chars[j] == '>' {
                let tag: String = chars[start + 1..j].iter().collect();
                let end = j + 1;
                if let Some(expanded) = try_expand_tag(&tag) {
                    flush(&mut out, &chars, base, run_start, start);
                    out.push(Spanned { text: expanded, src: (base + start)..(base + end) });
                    run_start = end;
                    i = end;
                    continue;
                }
                // no expansion — re-emitted text equals the input slice, no-op.
                i = end;
                continue;
            }
            // no closing '>' — also a no-op for this range.
            i = j;
            continue;
        }
        i += 1;
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

// ---------------------------------------------------------------------------
// Pass 4: year range expansion
// ---------------------------------------------------------------------------

#[cfg(test)]
fn expand_year_ranges(text: &str) -> String {
    flatten(&expand_year_ranges_spanned(spanned_from_input(text)))
}

// ---------------------------------------------------------------------------
// Span-mapping proof of concept (not yet wired into `normalize()`).
//
// Goal: map a char offset in normalized output text back to the char range
// in the original input it was derived from — needed so F5 (which
// normalizes before synthesis) can translate forced-alignment word
// timestamps, which land on normalized-text offsets, back onto the original
// sentence text used for annotation matching. See dev/annotation.md Stage 3.
//
// Approach: thread `Spanned` chunks through each pass instead of a flat
// `String`. Each chunk's `src` range always refers to char offsets in the
// *original* input, never the previous pass's output, so spans compose
// correctly across multiple passes without re-deriving anything.
//
// Granularity is per-chunk, not per-char: an expanded chunk (e.g. "1976 to
// 77") maps as a whole back to its source range (e.g. "1976-77"); we don't
// try to say which output character corresponds to which input character
// within an expansion. That's coarser than per-char tracking but sufficient
// for word-boundary lookups (the annotation-matching use case), and much
// simpler to get right across many transform rules.
//
// Not yet called from `normalize()` — only 2 of ~13 passes are converted so
// far (proof of concept). Exercised by tests below; `#[allow(dead_code)]`
// until the remaining passes are converted and this gets wired in.
// ---------------------------------------------------------------------------

/// A run of normalized text tagged with the char-index range in the
/// original (pre-normalization) input it was derived from.
#[derive(Debug, Clone, PartialEq)]
struct Spanned {
    text: String,
    src: std::ops::Range<usize>,
}

/// Wrap the whole input as a single unprocessed chunk, ready for the first pass.
fn spanned_from_input(text: &str) -> Vec<Spanned> {
    vec![Spanned { text: text.to_owned(), src: 0..text.chars().count() }]
}

/// True if `chunk`'s text is still in 1:1 char-length correspondence with
/// its source range — i.e. no earlier pass has expanded/contracted it yet.
///
/// Scan-and-subdivide passes (the "flush + expand" ones, plus tokenize and
/// punctuated-overrides) must treat a non-1:1 chunk as opaque/atomic rather
/// than re-scanning into it: once a chunk's text has grown or shrunk from
/// its original source range, local char offsets within that text no
/// longer correspond to offsets in the original source, so computing
/// `base + local_offset` would silently produce wrong spans. This also
/// matches the chunk-granularity design intent — text produced by an
/// earlier expansion (e.g. "seven one two") is already finalized spoken
/// form and shouldn't be re-split and re-matched by later passes.
fn is_raw(chunk: &Spanned) -> bool {
    chunk.text.chars().count() == chunk.src.len()
}

/// Concatenate chunks back into a flat string — equivalent to what the
/// unspanned pass would have returned.
fn flatten(chunks: &[Spanned]) -> String {
    chunks.iter().map(|c| c.text.as_str()).collect()
}

/// Map a char offset into the flattened/normalized text to the source char
/// range of the chunk it falls in, or `None` if out of bounds.
fn source_span_at(chunks: &[Spanned], normalized_offset: usize) -> Option<std::ops::Range<usize>> {
    let mut pos = 0;
    for chunk in chunks {
        let chunk_len = chunk.text.chars().count();
        if normalized_offset < pos + chunk_len {
            return Some(chunk.src.clone());
        }
        pos += chunk_len;
    }
    None
}

/// Span-aware version of `expand_year_ranges` — same transformation as the
/// unspanned version above, but emits `Spanned` chunks instead of a flat
/// `String`. Each chunk preceding/following an expansion keeps its
/// inherited source range; each expanded "N to M" chunk gets the source
/// range of the full digit-hyphen-digit run it replaced.
fn expand_year_ranges_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| expand_year_ranges_chunk(&chunk)).collect()
}

fn expand_year_ranges_chunk(chunk: &Spanned) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() { i += 1; }
            if i < chars.len() && chars[i] == '-' {
                let after_hyphen = i + 1;
                if after_hyphen < chars.len() && chars[after_hyphen].is_ascii_digit() {
                    let mut end = after_hyphen;
                    while end < chars.len() && chars[end].is_ascii_digit() { end += 1; }
                    if end >= chars.len() || !chars[end].is_alphabetic() {
                        flush(&mut out, &chars, base, run_start, start);
                        // Spell out both halves here, not just insert "to" —
                        // once this chunk is expanded it's no longer raw, so
                        // Pass 7's bare-number rule can never reach these
                        // digits afterward (chunk-granularity design). Left
                        // bare, they'd be invisible to forced alignment
                        // entirely, not just unspoken awkwardly.
                        let first: String = chars[start..i].iter().collect();
                        let second: String = chars[after_hyphen..end].iter().collect();
                        let expanded = format!(
                            "{} to {}",
                            spell_bare_digit_group(&first).unwrap_or(first),
                            spell_bare_digit_group(&second).unwrap_or(second),
                        );
                        out.push(Spanned { text: expanded, src: (base + start)..(base + end) });
                        run_start = end;
                        i = end;
                        continue;
                    }
                }
            }
            // not a year range — digit run stays queued as unprocessed text
        } else {
            i += 1;
        }
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

/// Span-aware replacement of hyphens with spaces — the trivial case: every
/// output char maps 1:1 to the same-position source char, so chunk source
/// ranges pass through unchanged. Superseded in the real pipeline by the
/// inline hyphen→space handling in `tokenize_chunk`; kept as the original
/// proof-of-concept test fixture for the 1:1-transform shape.
#[cfg(test)]
fn hyphens_to_spaces_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().map(|chunk| {
        let text: String = chunk.text.chars().map(|c| if c == '-' { ' ' } else { c }).collect();
        Spanned { text, src: chunk.src }
    }).collect()
}

/// Span-aware bracket stripping. Removing chars shrinks the text, so this
/// must subdivide via `replace_literal_spanned` (one pass per bracket char,
/// matching the unspanned filter's effect) rather than blindly mapping the
/// whole chunk — a naive map would silently break the text/src length
/// invariant for the *rest* of the chunk's text, making it invisible to any
/// later pass that needs to scan it (see `is_raw`).
fn strip_brackets_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    let mut chunks = chunks;
    for bracket in ['(', ')', '<', '>', '[', ']'] {
        chunks = replace_literal_spanned(chunks, &bracket.to_string(), "");
    }
    chunks
}

/// Span-aware ellipsis stripping — same reasoning as bracket stripping:
/// removal shrinks text, so it must subdivide rather than map. Mirrors the
/// unspanned version's exact sequence of four literal replacements.
fn replace_ellipsis_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    let chunks = replace_literal_spanned(chunks, " ...", "");
    let chunks = replace_literal_spanned(chunks, " \u{2026}", "");
    let chunks = replace_literal_spanned(chunks, "...", "");
    replace_literal_spanned(chunks, "\u{2026}", "")
}

/// Span-aware version of `replace_identifier_dots`. Each matched `.` (a
/// single source char) expands to the 5-char " dot " — same flush/expand
/// shape as `expand_year_ranges_chunk`, just with a 1-char source range per
/// expansion instead of a multi-char one.
fn replace_identifier_dots_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| replace_identifier_dots_chunk(&chunk)).collect()
}

fn replace_identifier_dots_chunk(chunk: &Spanned) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    for i in 0..chars.len() {
        if chars[i] == '.'
            && (i == 0 || chars[i - 1] != '.')
            && i + 1 < chars.len() && chars[i + 1].is_alphanumeric()
        {
            flush(&mut out, &chars, base, run_start, i);
            out.push(Spanned { text: " dot ".to_owned(), src: (base + i)..(base + i + 1) });
            run_start = i + 1;
        }
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

/// Replace em-dashes and double-hyphens with a comma so TTS gets a clean
/// pause cue instead of extra whitespace that causes vocalisation artifacts.
#[cfg(test)]
fn replace_em_dash_and_double_hyphen(text: &str) -> String {
    flatten(&replace_em_dash_and_double_hyphen_spanned(spanned_from_input(text)))
}

/// Span-aware version. " -- " (4 chars) -> ", " (2 chars) shrinks text, so
/// — like bracket/ellipsis stripping — this must subdivide via
/// `replace_literal_spanned` rather than map the whole chunk, or the
/// length mismatch would make the rest of the chunk's text invisible to
/// later passes (see `is_raw`). Mirrors the unspanned version's two
/// sequential literal replacements.
fn replace_em_dash_and_double_hyphen_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    let chunks = replace_literal_spanned(chunks, "\u{2014}", ",");
    replace_literal_spanned(chunks, " -- ", ", ")
}

// ---------------------------------------------------------------------------
// Pass 7: bare number expansion (comma-grouped and digit-group)
// ---------------------------------------------------------------------------

/// Expand comma-grouped numbers like "2,000" or "100,000" into words
/// ("two thousand", "one hundred thousand"). Requires each group after the
/// first comma to have exactly 3 digits, and the whole number not to be
/// adjacent to other digits/letters (so "Ref-1,000" style codes are left
/// alone).
#[cfg(test)]
fn expand_comma_numbers(text: &str) -> String {
    flatten(&expand_comma_numbers_spanned(spanned_from_input(text)))
}

/// Span-aware version of `expand_comma_numbers` — same flush/expand shape
/// as `expand_year_ranges_chunk`: an unexpanded digit run (no valid
/// comma-grouping found) falls through and stays queued in the pending
/// unprocessed range; an expanded run gets the source range of the whole
/// digit-comma sequence it replaced.
fn expand_comma_numbers_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| expand_comma_numbers_chunk(&chunk)).collect()
}

fn expand_comma_numbers_chunk(chunk: &Spanned) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    while i < chars.len() {
        if chars[i].is_ascii_digit() && (i == 0 || !chars[i - 1].is_ascii_digit()) {
            let start = i;
            let mut j = i;
            while j < chars.len() && chars[j].is_ascii_digit() { j += 1; }
            let mut end = j;
            while end < chars.len() && chars[end] == ','
                && end + 3 < chars.len()
                && chars[end + 1..end + 4].iter().all(|c| c.is_ascii_digit())
                && (end + 4 == chars.len() || !chars[end + 4].is_ascii_digit())
            {
                end += 4;
            }
            if end > j {
                let digits: String = chars[start..end].iter().filter(|c| **c != ',').collect();
                if let Ok(n) = digits.parse::<u64>() {
                    flush(&mut out, &chars, base, run_start, start);
                    out.push(Spanned { text: number_to_words(n), src: (base + start)..(base + end) });
                    run_start = end;
                    i = end;
                    continue;
                }
            }
            // Not comma-grouped. A bare 1-4 digit number gets spelled out
            // ("5" -> "five", "42" -> "forty two", "560" -> "five sixty",
            // "1976" -> "nineteen seventy six") rather than being read as
            // raw digits — which TTS tends to garble, and which forced
            // alignment can't time-align at all (its vocabulary has no
            // digit characters, so bare numbers get silently dropped).
            // Also excluded here: a digit run touching a letter on either
            // side ("4b", "14B", "v2", "512K") — not skipped entirely, just
            // deferred: split_alphanumeric (in process_token) splits these
            // later in the pipeline and spells out each digit run itself,
            // so the number still gets spelled, just after the letter is
            // separated off rather than here. (Leading-zero exclusion is
            // handled inside `spell_bare_digit_group` itself.)
            let touches_letter = (start > 0 && chars[start - 1].is_alphabetic())
                || (j < chars.len() && chars[j].is_alphabetic());
            if !touches_letter {
                let digits: String = chars[start..j].iter().collect();
                if let Some(words) = spell_bare_digit_group(&digits) {
                    flush(&mut out, &chars, base, run_start, start);
                    out.push(Spanned { text: words, src: (base + start)..(base + j) });
                    run_start = j;
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

/// Spell out a bare 3-digit number "digit-group" style: the hundreds digit
/// read alone, then the remaining two digits read as a unit. E.g. "560" ->
/// "five sixty", "501" -> "five oh one" (leading zero in the last two
/// digits, address/phone convention), "500" -> "five hundred" (round).
fn three_digit_group_words(digits: &str) -> String {
    let mut chars = digits.chars();
    let hundreds = digit_word(chars.next().expect("3-digit number"));
    let rest = chars.as_str(); // remaining 2 digits, e.g. "60", "01", "00"

    if rest == "00" {
        format!("{hundreds} hundred")
    } else if rest.starts_with('0') {
        let ones = digit_word(rest.chars().nth(1).expect("2-digit remainder"));
        format!("{hundreds} oh {ones}")
    } else {
        let rest_n: u64 = rest.parse().expect("2 ascii digits");
        format!("{hundreds} {}", number_to_words(rest_n))
    }
}

/// Spell out a bare 4-digit number "digit-group" style, as years and
/// addresses are normally read: two 2-digit groups, each read as its own
/// number. E.g. "1976" -> "nineteen seventy six", "2010" -> "twenty ten".
///
/// Special cases for round numbers (second group "00"):
/// - Round thousand ("2000", "3000" — first group itself ends in 0) reads
///   as a single cardinal: "two thousand", "three thousand". Splitting
///   these into groups would give the wrong "twenty hundred".
/// - Round hundred-within-thousand ("1900", "2300") reads as
///   "<first group> hundred": "nineteen hundred", "twenty three hundred".
///
/// A leading zero in the second group ("1905") reads as "oh <digit>":
/// "nineteen oh five".
fn four_digit_group_words(digits: &str) -> String {
    let first = &digits[0..2];
    let second = &digits[2..4];
    let group_words = |s: &str| number_to_words(s.parse::<u64>().expect("2 ascii digits"));

    if second == "00" {
        if first.ends_with('0') {
            let n: u64 = digits.parse().expect("4 ascii digits");
            number_to_words(n)
        } else {
            format!("{} hundred", group_words(first))
        }
    } else if second.starts_with('0') {
        let ones = digit_word(second.chars().nth(1).expect("2-digit group"));
        format!("{} oh {ones}", group_words(first))
    } else {
        format!("{} {}", group_words(first), group_words(second))
    }
}

/// Spell out a bare digit-only string using the same convention as the
/// generic bare-number rule (Pass 7): 1-2 digits via `number_to_words`,
/// 3 via `three_digit_group_words`, 4 via `four_digit_group_words`.
/// Returns `None` for forms this convention doesn't cover — 5+ digits, or
/// a leading-zero run longer than 1 char (those are IDs, spelled
/// digit-by-digit elsewhere in `process_token`) — so callers can fall back
/// to leaving the digits bare.
///
/// Shared by Pass 7 itself and by `expand_year_ranges_chunk` (Pass 4),
/// which needs to spell out both halves of a range like "1973-76" itself
/// rather than leaving bare digits for Pass 7 to find — once Pass 4 expands
/// "1973-76" into "1973 to 76", that text is no longer raw, so Pass 7's
/// scan correctly skips it (per the chunk-granularity design), and the
/// embedded digits would otherwise stay bare forever: not just unspoken
/// awkwardly, but invisible to forced alignment (whose vocabulary has no
/// digit characters), entirely dropping that word from alignment results.
fn spell_bare_digit_group(digits: &str) -> Option<String> {
    let len = digits.chars().count();
    if !matches!(len, 1 | 2 | 3 | 4) { return None; }
    if len > 1 && digits.starts_with('0') { return None; }
    Some(match len {
        1 | 2 => number_to_words(digits.parse().expect("1-2 ascii digits")),
        3 => three_digit_group_words(digits),
        _ => four_digit_group_words(digits),
    })
}

// ---------------------------------------------------------------------------
// Pass 1: handle punctuated overrides before token processing, so they can
// match across punctuation (e.g. `e.g.`).
// ---------------------------------------------------------------------------

#[cfg(test)]
fn apply_punctuated_overrides(text: &str, overrides: &HashMap<String, String>) -> String {
    flatten(&apply_punctuated_overrides_spanned(spanned_from_input(text), overrides))
}

/// Span-aware version of `apply_punctuated_overrides`. Each override key is
/// applied as its own full pass over the chunk list (mirroring the unspanned
/// version's one-pass-per-key loop), so spans stay correctly anchored even
/// when multiple override keys apply to overlapping-looking text in sequence.
fn apply_punctuated_overrides_spanned(chunks: Vec<Spanned>, overrides: &HashMap<String, String>) -> Vec<Spanned> {
    let mut chunks = chunks;
    for (from, to) in overrides.iter().filter(|(k, _)| !k.chars().all(|c| c.is_alphanumeric() || c == '\'')) {
        chunks = chunks.into_iter().flat_map(|chunk| apply_one_override_chunk(&chunk, from, to)).collect();
    }
    chunks
}

fn apply_one_override_chunk(chunk: &Spanned, from: &str, to: &str) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let from_chars: Vec<char> = from.chars().collect();
    let from_len = from_chars.len();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    // Require a word boundary before the match when `from` starts with an
    // alphanumeric char, so e.g. "p." doesn't fire inside "Scholarship."
    // (the "p" + sentence-final "." just happens to spell the key).
    let needs_boundary = from_chars.first().is_some_and(|c| c.is_alphanumeric());

    while i < chars.len() {
        let matches = from_len > 0 && i + from_len <= chars.len()
            && chars[i..i + from_len].iter().zip(from_chars.iter())
                .all(|(a, b)| a.to_lowercase().eq(b.to_lowercase()))
            && (!needs_boundary || i == 0 || !chars[i - 1].is_alphanumeric());
        if matches {
            flush(&mut out, &chars, base, run_start, i);
            out.push(Spanned { text: to.to_owned(), src: (base + i)..(base + i + from_len) });
            run_start = i + from_len;
            i += from_len;
        } else {
            i += 1;
        }
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

/// Generic span-aware exact-match literal substring replace (case-sensitive,
/// unlike `apply_one_override_chunk`). Used by any pass that needs to
/// replace or remove (`to = ""`) a fixed substring while staying scannable
/// by later passes — i.e. instead of blindly mapping a chunk's whole text
/// (which silently breaks the text/src length invariant the moment the
/// replacement isn't 1:1), this flushes/subdivides like the other
/// expansion passes so untouched text around each match stays "raw" and
/// available for further matching.
fn replace_literal_spanned(chunks: Vec<Spanned>, from: &str, to: &str) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| replace_literal_chunk(&chunk, from, to)).collect()
}

fn replace_literal_chunk(chunk: &Spanned, from: &str, to: &str) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let from_chars: Vec<char> = from.chars().collect();
    let from_len = from_chars.len();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    while i < chars.len() {
        if from_len > 0 && i + from_len <= chars.len() && chars[i..i + from_len] == from_chars[..] {
            flush(&mut out, &chars, base, run_start, i);
            if !to.is_empty() {
                out.push(Spanned { text: to.to_owned(), src: (base + i)..(base + i + from_len) });
            }
            run_start = i + from_len;
            i += from_len;
        } else {
            i += 1;
        }
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

// ---------------------------------------------------------------------------
// Pass 11: token processing
// ---------------------------------------------------------------------------

fn process_token(token: &str, overrides: &HashMap<String, String>) -> String {
    if let Some(replacement) = overrides.get(&token.to_lowercase()) {
        return replacement.clone();
    }

    let has_digit = token.chars().any(|c| c.is_ascii_digit());
    let has_alpha = token.chars().any(|c| c.is_alphabetic());
    if has_digit && has_alpha {
        return split_alphanumeric(token);
    }

    if has_alpha && !has_digit {
        return process_alpha_token(token);
    }

    // Leading-zero numbers ("0609", "069") are IDs, not magnitudes — spell
    // each digit as a word so TTS reads them clearly.
    if has_digit && token.len() > 1 && token.starts_with('0') {
        return token.chars().map(digit_word).collect::<Vec<_>>().join(" ");
    }

    token.to_owned()
}

/// Splits an alphanumeric token into digit/letter runs, spelling out each
/// digit run as words (e.g. "512K" -> "five twelve K", "4c2" -> "four c
/// two") instead of leaving raw digits — raw digits are hard for TTS to
/// pronounce reliably and forced alignment has no digit characters in its
/// vocabulary, so they'd otherwise go untimed. `spell_bare_digit_group`
/// handles the common 1-4 digit cases the same way bare numbers elsewhere
/// in the pipeline are spelled; runs it doesn't cover (longer runs, or a
/// leading zero with len > 1, e.g. an ID like "007") fall back to spelling
/// each digit individually.
fn split_alphanumeric(token: &str) -> String {
    let mut groups: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut prev_kind: Option<bool> = None;

    for ch in token.chars() {
        let is_digit = ch.is_ascii_digit();
        if let Some(prev) = prev_kind {
            if prev != is_digit {
                groups.push(std::mem::take(&mut current));
            }
        }
        current.push(ch);
        prev_kind = Some(is_digit);
    }
    if !current.is_empty() { groups.push(current); }

    groups.into_iter()
        .map(|g| {
            if g.chars().all(|c| c.is_ascii_digit()) {
                spell_bare_digit_group(&g)
                    .unwrap_or_else(|| g.chars().map(digit_word).collect::<Vec<_>>().join(" "))
            } else {
                g
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Spell out a number in words, e.g. 1999 -> "one thousand nine hundred
/// ninety nine". Drops num2words' hyphens ("ninety-nine") and "and"
/// ("hundred and ninety-nine") to match this normalizer's plain
/// space-separated style.
fn number_to_words(n: u64) -> String {
    let words = num2words::Num2Words::new(n)
        .to_words()
        .unwrap_or_else(|_| n.to_string());
    words
        .replace('-', " ")
        .split(' ')
        .filter(|w| *w != "and")
        .collect::<Vec<_>>()
        .join(" ")
}

fn process_alpha_token(token: &str) -> String {
    let (stem, suffix) = if let Some(s) = token.strip_suffix("'s")
        .or_else(|| token.strip_suffix("'S"))
    {
        (s, &token[s.len()..])
    } else {
        (token, "")
    };

    let alpha_count = stem.chars().filter(|c| c.is_alphabetic()).count();
    let all_caps = stem.chars().filter(|c| c.is_alphabetic()).all(|c| c.is_uppercase());

    // Roman numeral check: skip single chars (I, V, X etc. are too ambiguous).
    // Only all-caps stems are converted — lowercase Roman numerals ("xxx",
    // "yy") are indistinguishable from lowercase placeholder strings (e.g.
    // "(DDD,xxx,bb)" in authorship.txt), which are far more common in our
    // target documents. Disambiguating "xiv" (= 14) from a placeholder would
    // need per-document overrides; not implemented.
    if alpha_count >= 2 && all_caps {
        if let Some(n) = roman::from(stem) {
            let words = number_to_words(n as u64);
            return if suffix.is_empty() { words } else { format!("{words}{}", suffix.to_lowercase()) };
        }
    }

    if all_caps {
        if alpha_count <= 3 {
            let spelled: Vec<String> = stem.chars().map(|c| c.to_string()).collect();
            let result = spelled.join(" ");
            if suffix.is_empty() { result } else { format!("{}{}", result, suffix.to_lowercase()) }
        } else {
            // Title-case: capitalize first letter so "GENERAL" → "General"
            // rather than "general". Sounds the same to TTS but reads more
            // naturally and avoids issues when preceded by punctuation (e.g. "A. General").
            let mut chars = stem.chars();
            let titled = match chars.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
            };
            format!("{}{}", titled, suffix.to_lowercase())
        }
    } else {
        token.to_owned()
    }
}

// ---------------------------------------------------------------------------
// Pass 11: tokenize on word boundaries, process each token, replace
// remaining hyphens with spaces.
// ---------------------------------------------------------------------------

#[cfg(test)]
fn tokenize(text: &str, overrides: &HashMap<String, String>) -> String {
    flatten(&tokenize_spanned(spanned_from_input(text), overrides))
}

/// Span-aware version of `tokenize`. Each token's whole char range becomes
/// the source span for whatever `process_token` outputs (per the
/// chunk-granularity design — we don't try to map individual output words
/// within a multi-word expansion back to individual input chars). Each
/// separator char (including hyphen→space) maps 1:1, like
/// `hyphens_to_spaces_spanned`.
fn tokenize_spanned(chunks: Vec<Spanned>, overrides: &HashMap<String, String>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| tokenize_chunk(&chunk, overrides)).collect()
}

fn tokenize_chunk(chunk: &Spanned, overrides: &HashMap<String, String>) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut tok_start: Option<usize> = None;

    fn flush_token(out: &mut Vec<Spanned>, chars: &[char], base: usize, tok_start: &mut Option<usize>, end: usize, overrides: &HashMap<String, String>) {
        if let Some(start) = tok_start.take() {
            let token: String = chars[start..end].iter().collect();
            let processed = process_token(&token, overrides);
            out.push(Spanned { text: processed, src: (base + start)..(base + end) });
        }
    }

    for i in 0..chars.len() {
        let ch = chars[i];
        if ch.is_alphanumeric() || ch == '\'' {
            if tok_start.is_none() { tok_start = Some(i); }
        } else {
            flush_token(&mut out, &chars, base, &mut tok_start, i, overrides);
            let out_ch = if ch == '-' { ' ' } else { ch };
            out.push(Spanned { text: out_ch.to_string(), src: (base + i)..(base + i + 1) });
        }
    }
    flush_token(&mut out, &chars, base, &mut tok_start, chars.len(), overrides);
    out
}

// ---------------------------------------------------------------------------
// Pass 2: strip short inline quotes
// ---------------------------------------------------------------------------

#[cfg(test)]
fn strip_short_quotes(text: &str) -> String {
    flatten(&strip_short_quotes_spanned(spanned_from_input(text)))
}

/// Span-aware version of `strip_short_quotes`.
///
/// Unlike every other `*_spanned` pass, this one can't process chunks
/// independently: `apply_punctuated_overrides_spanned` runs first (by
/// design — see `normalize_with_spans`), and an override match can split
/// what was one chunk into [prefix, expanded-replacement, suffix], landing
/// an opening and closing quote in two *different* chunks with a non-raw
/// expansion between them. A per-chunk scan would never see the matching
/// closing quote. So this scans the flattened text once to find quote
/// pairs to remove, then deletes exactly those two char positions from
/// whichever chunks they fall in — every other chunk (including non-raw
/// ones in between) is passed through completely unmodified, so this never
/// needs to (and doesn't) recompute spans for already-expanded text.
fn strip_short_quotes_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    let flat: Vec<char> = flatten(&chunks).chars().collect();

    let mut to_remove: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut i = 0;
    while i < flat.len() {
        if flat[i] == '"' {
            let content_start = i + 1;
            let mut j = content_start;
            while j < flat.len() && flat[j] != '"' { j += 1; }
            if j < flat.len() {
                let word_count = flat[content_start..j].iter().collect::<String>()
                    .split_whitespace().count();
                if word_count <= 5 {
                    to_remove.insert(i);
                    to_remove.insert(j);
                    i = j + 1;
                    continue;
                }
            }
        }
        i += 1;
    }

    if to_remove.is_empty() { return chunks; }

    let mut out = Vec::new();
    let mut flat_pos = 0;
    for chunk in chunks {
        let chunk_chars: Vec<char> = chunk.text.chars().collect();
        let mut piece_start = 0;
        for local_i in 0..chunk_chars.len() {
            if to_remove.contains(&(flat_pos + local_i)) {
                if local_i > piece_start {
                    out.push(subchunk(&chunk, &chunk_chars, piece_start, local_i));
                }
                piece_start = local_i + 1;
            }
        }
        if piece_start < chunk_chars.len() {
            out.push(subchunk(&chunk, &chunk_chars, piece_start, chunk_chars.len()));
        }
        flat_pos += chunk_chars.len();
    }
    out
}

/// Build a sub-piece [start, end) of `chunk`'s text. If `chunk` is still
/// raw, the sub-piece gets a precisely narrowed source range; if it's
/// already non-raw (expanded by an earlier pass), precise sub-ranges
/// aren't meaningful, so the whole piece inherits the parent's full source
/// range (per the chunk-granularity design for already-expanded text).
fn subchunk(chunk: &Spanned, chunk_chars: &[char], start: usize, end: usize) -> Spanned {
    let text: String = chunk_chars[start..end].iter().collect();
    let src = if is_raw(chunk) {
        (chunk.src.start + start)..(chunk.src.start + end)
    } else {
        chunk.src.clone()
    };
    Spanned { text, src }
}

// ---------------------------------------------------------------------------
// Pass 6: spell Item/reference numbers digit-by-digit
// ---------------------------------------------------------------------------

#[cfg(test)]
fn spell_item_numbers(text: &str) -> String {
    flatten(&spell_item_numbers_spanned(spanned_from_input(text)))
}

/// Span-aware version of `spell_item_numbers`. The matched range covers
/// "Item" + whitespace + digits as one source span, since the expansion
/// ("Item one two three four") isn't word-for-word aligned to the original
/// digits — per the chunk-granularity design, callers get the whole
/// matched range, not a per-digit breakdown.
fn spell_item_numbers_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| spell_item_numbers_chunk(&chunk)).collect()
}

fn spell_item_numbers_chunk(chunk: &Spanned) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    while i < chars.len() {
        if i + 4 <= chars.len()
            && chars[i] == 'I' && chars[i+1] == 't' && chars[i+2] == 'e' && chars[i+3] == 'm'
            && (i == 0 || !chars[i-1].is_alphanumeric())
        {
            let mut j = i + 4;
            while j < chars.len() && chars[j].is_whitespace() { j += 1; }
            let digit_start = j;
            while j < chars.len() && chars[j].is_ascii_digit() { j += 1; }
            let digit_count = j - digit_start;
            if digit_count >= 4 && (j == chars.len() || !chars[j].is_alphanumeric()) {
                flush(&mut out, &chars, base, run_start, i);
                let mut expanded = String::from("Item ");
                for (k, &c) in chars[digit_start..j].iter().enumerate() {
                    if k > 0 { expanded.push(' '); }
                    expanded.push_str(digit_word(c));
                }
                out.push(Spanned { text: expanded, src: (base + i)..(base + j) });
                run_start = j;
                i = j;
                continue;
            }
        }
        i += 1;
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

fn digit_word(c: char) -> &'static str {
    match c {
        '0' => "zero", '1' => "one",  '2' => "two",   '3' => "three", '4' => "four",
        '5' => "five", '6' => "six",  '7' => "seven",  '8' => "eight", '9' => "nine",
        _   => "",
    }
}

// ---------------------------------------------------------------------------
// Pass 8: expand journal links — (AUGMENT,71279,) → "Augment 7 1 2 7 9"
// ---------------------------------------------------------------------------

/// Matches optional open bracket, 2+ uppercase letters, comma, digits, comma,
/// optional close bracket. e.g. (AUGMENT,71279,) or <OAD,2237,>
#[cfg(test)]
fn expand_journal_links(text: &str) -> String {
    flatten(&expand_journal_links_spanned(spanned_from_input(text)))
}

/// If a journal-link pattern starts at `i`, return (name, digits, end-index).
fn match_journal_link(chars: &[char], i: usize) -> Option<(String, String, usize)> {
    let name_start = if matches!(chars[i], '(' | '<') { i + 1 } else { i };
    let mut j = name_start;
    while j < chars.len() && chars[j].is_ascii_uppercase() { j += 1; }
    if j - name_start < 2 || j >= chars.len() || chars[j] != ',' { return None; }

    let digit_start = j + 1;
    let mut k = digit_start;
    while k < chars.len() && chars[k].is_ascii_digit() { k += 1; }
    if k == digit_start || k >= chars.len() || chars[k] != ',' { return None; }

    let mut end = k + 1;
    if end < chars.len() && matches!(chars[end], ')' | '>') { end += 1; }

    Some((
        chars[name_start..j].iter().collect(),
        chars[digit_start..k].iter().collect(),
        end,
    ))
}

/// Span-aware version of `expand_journal_links` — same flush/expand shape
/// as the other multi-char expansions; the matched range covers the whole
/// link (brackets included) as one source span.
fn expand_journal_links_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| expand_journal_links_chunk(&chunk)).collect()
}

fn expand_journal_links_chunk(chunk: &Spanned) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    while i < chars.len() {
        if let Some((name, digits, end)) = match_journal_link(&chars, i) {
            flush(&mut out, &chars, base, run_start, i);
            let spoken_name = process_alpha_token(&name);
            let spelled: Vec<&str> = digits.chars().map(digit_word).collect();
            let expanded = format!("{} {}", spoken_name, spelled.join(" "));
            out.push(Spanned { text: expanded, src: (base + i)..(base + end) });
            run_start = end;
            i = end;
        } else {
            i += 1;
        }
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

// ---------------------------------------------------------------------------
// Pass 9: expand US state postal abbreviations ("City, ST" pattern only)
// ---------------------------------------------------------------------------

fn state_full_name(abbr: &str) -> Option<&'static str> {
    Some(match abbr {
        "AL" => "Alabama", "AK" => "Alaska", "AZ" => "Arizona", "AR" => "Arkansas",
        "CA" => "California", "CO" => "Colorado", "CT" => "Connecticut", "DE" => "Delaware",
        "FL" => "Florida", "GA" => "Georgia", "HI" => "Hawaii", "ID" => "Idaho",
        "IL" => "Illinois", "IN" => "Indiana", "IA" => "Iowa", "KS" => "Kansas",
        "KY" => "Kentucky", "LA" => "Louisiana", "ME" => "Maine", "MD" => "Maryland",
        "MA" => "Massachusetts", "MI" => "Michigan", "MN" => "Minnesota", "MS" => "Mississippi",
        "MO" => "Missouri", "MT" => "Montana", "NE" => "Nebraska", "NV" => "Nevada",
        "NH" => "New Hampshire", "NJ" => "New Jersey", "NM" => "New Mexico", "NY" => "New York",
        "NC" => "North Carolina", "ND" => "North Dakota", "OH" => "Ohio", "OK" => "Oklahoma",
        "OR" => "Oregon", "PA" => "Pennsylvania", "RI" => "Rhode Island", "SC" => "South Carolina",
        "SD" => "South Dakota", "TN" => "Tennessee", "TX" => "Texas", "UT" => "Utah",
        "VT" => "Vermont", "VA" => "Virginia", "WA" => "Washington", "WV" => "West Virginia",
        "WI" => "Wisconsin", "WY" => "Wyoming", "DC" => "District of Columbia",
        _ => return None,
    })
}

/// Expand a two-letter state code only when it appears right after a comma
/// (the "City, ST" pattern), e.g. "Denver, CO" → "Denver, Colorado". Codes
/// elsewhere in the text (e.g. as ordinary words) are left untouched.
#[cfg(test)]
fn expand_state_abbrevs(text: &str) -> String {
    flatten(&expand_state_abbrevs_spanned(spanned_from_input(text)))
}

/// If a known "City, ST" state-code pattern starts at `i` (comma, one
/// whitespace char, two uppercase letters, word boundary), return the
/// state's full name. The matched span is always exactly 4 chars
/// (`,` + whitespace + 2 letters) when this returns `Some`.
fn match_state_abbrev(chars: &[char], i: usize) -> Option<&'static str> {
    if chars[i] != ',' || i + 3 >= chars.len() { return None; }
    if !chars[i + 1].is_whitespace() { return None; }
    if !chars[i + 2].is_ascii_uppercase() || !chars[i + 3].is_ascii_uppercase() { return None; }
    if i + 4 < chars.len() && is_word_char(chars[i + 4]) { return None; }

    let code: String = chars[i + 2..i + 4].iter().collect();
    state_full_name(&code)
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Span-aware version of `expand_state_abbrevs`. The matched 4-char span
/// (`,` + whitespace + 2-letter code) is the source range for the expanded
/// `, <Full Name>` chunk.
fn expand_state_abbrevs_spanned(chunks: Vec<Spanned>) -> Vec<Spanned> {
    chunks.into_iter().flat_map(|chunk| expand_state_abbrevs_chunk(&chunk)).collect()
}

fn expand_state_abbrevs_chunk(chunk: &Spanned) -> Vec<Spanned> {
    if !is_raw(chunk) { return vec![chunk.clone()]; }
    let chars: Vec<char> = chunk.text.chars().collect();
    let base = chunk.src.start;
    let mut out = Vec::new();
    let mut run_start = 0;
    let mut i = 0;

    fn flush(out: &mut Vec<Spanned>, chars: &[char], base: usize, run_start: usize, end: usize) {
        if end > run_start {
            out.push(Spanned {
                text: chars[run_start..end].iter().collect(),
                src: (base + run_start)..(base + end),
            });
        }
    }

    while i < chars.len() {
        if let Some(name) = match_state_abbrev(&chars, i) {
            flush(&mut out, &chars, base, run_start, i);
            out.push(Spanned { text: format!(", {name}"), src: (base + i)..(base + i + 4) });
            run_start = i + 4;
            i += 4;
        } else {
            i += 1;
        }
    }
    flush(&mut out, &chars, base, run_start, chars.len());
    out
}

// ---------------------------------------------------------------------------
// Pass 10: replace dots between alphanumeric chars with " dot "
// ---------------------------------------------------------------------------

#[cfg(test)]
fn replace_identifier_dots(text: &str) -> String {
    flatten(&replace_identifier_dots_spanned(spanned_from_input(text)))
}

// ---------------------------------------------------------------------------
// Pass 13: replace ellipses with newlines
// ---------------------------------------------------------------------------

#[cfg(test)]
fn replace_ellipsis(text: &str) -> String {
    flatten(&replace_ellipsis_spanned(spanned_from_input(text)))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roman_numerals_converted() {
        assert_eq!(process_alpha_token("II"),    "two");
        assert_eq!(process_alpha_token("III"),   "three");
        assert_eq!(process_alpha_token("IV"),    "four");
        assert_eq!(process_alpha_token("VIII"),  "eight");
        assert_eq!(process_alpha_token("IX"),    "nine");
        assert_eq!(process_alpha_token("XIV"),   "fourteen");
        assert_eq!(process_alpha_token("XIX"),   "nineteen");
        assert_eq!(process_alpha_token("XX"),    "twenty");
        assert_eq!(process_alpha_token("XXI"),   "twenty one");
        assert_eq!(process_alpha_token("XLII"),  "forty two");
        assert_eq!(process_alpha_token("XVIII"), "eighteen");
        assert_eq!(process_alpha_token("MCMXCIX"), "one thousand nine hundred ninety nine");
    }

    #[test]
    #[ignore = "lowercase Roman numerals disabled: indistinguishable from \
                placeholder strings like 'xxx'/'yy' in authorship.txt; \
                would need per-document overrides to do both correctly"]
    fn roman_lowercase_converted() {
        assert_eq!(process_alpha_token("ii"),    "two");
        assert_eq!(process_alpha_token("iii"),   "three");
        assert_eq!(process_alpha_token("iv"),    "four");
        assert_eq!(process_alpha_token("viii"),  "eight");
        assert_eq!(process_alpha_token("xiv"),   "fourteen");
        assert_eq!(process_alpha_token("xcix"),  "ninety nine");
    }

    #[test]
    fn lowercase_placeholder_strings_unchanged() {
        // "xxx", "yy" etc. as Journal-item placeholders (authorship.txt) must
        // not be read as Roman numerals.
        assert_eq!(process_alpha_token("xxx"), "xxx");
        assert_eq!(process_alpha_token("yy"),  "yy");
        assert_eq!(process_alpha_token("mix"), "mix");
    }

    #[test]
    fn roman_single_chars_not_converted() {
        // Single Roman-numeral chars are too ambiguous — left to existing all-caps logic.
        assert_eq!(process_alpha_token("I"), "I");
        assert_eq!(process_alpha_token("V"), "V");
        assert_eq!(process_alpha_token("X"), "X");
        assert_eq!(process_alpha_token("i"), "i");
    }

    #[test]
    fn non_roman_allcaps_unchanged() {
        // These fail the canonical re-encoding check and fall through to all-caps handling.
        assert_eq!(process_alpha_token("CIVIL"),  "Civil");
        assert_eq!(process_alpha_token("MILL"),   "Mill");
        // MIX is a genuine Roman numeral (1009 = M + IX) — converted, not lowercased.
        assert_eq!(process_alpha_token("MIX"),    "one thousand nine");
    }

    #[test]
    fn roman_possessive() {
        assert_eq!(process_alpha_token("VIII's"), "eight's");
    }

    #[test]
    fn number_to_words_spot_checks() {
        assert_eq!(number_to_words(1),      "one");
        assert_eq!(number_to_words(14),     "fourteen");
        assert_eq!(number_to_words(20),     "twenty");
        assert_eq!(number_to_words(21),     "twenty one");
        assert_eq!(number_to_words(100),    "one hundred");
        assert_eq!(number_to_words(1999),   "one thousand nine hundred ninety nine");
        assert_eq!(number_to_words(2000),   "two thousand");
        assert_eq!(number_to_words(100000), "one hundred thousand");
    }

    #[test]
    fn leading_zero_numbers_spelled_as_words() {
        assert_eq!(process_token("0609", &HashMap::new()), "zero six zero nine");
        assert_eq!(process_token("069",  &HashMap::new()), "zero six nine");
        assert_eq!(process_token("012",  &HashMap::new()), "zero one two");
        // single "0" is not an "ID" — leave alone.
        assert_eq!(process_token("0",    &HashMap::new()), "0");
    }

    #[test]
    fn short_quotes_stripped() {
        assert_eq!(strip_short_quotes(r#""foo bar""#), "foo bar");
        assert_eq!(strip_short_quotes(r#""one two three four five""#), "one two three four five");
        // 6 words — keep quotes
        assert_eq!(strip_short_quotes(r#""one two three four five six""#),
                   r#""one two three four five six""#);
        assert_eq!(strip_short_quotes(r#"He said "hello" and left"#), "He said hello and left");
    }

    #[test]
    fn identifier_dots_replaced() {
        assert_eq!(replace_identifier_dots("4b.l"),   "4b dot l");
        assert_eq!(replace_identifier_dots("4b.dt"),  "4b dot dt");
        assert_eq!(replace_identifier_dots("Ref.l"),  "Ref dot l");
        assert_eq!(replace_identifier_dots("3.14"),   "3 dot 14");
        // standalone: space before dot, alphanumeric after — also replaced
        assert_eq!(replace_identifier_dots(" .l "),   "  dot l ");
        // dot at end of sentence — unchanged (nothing alphanumeric after)
        assert_eq!(replace_identifier_dots("end."),   "end.");
        assert_eq!(replace_identifier_dots(". foo"),  ". foo");
        // ellipsis — middle dots preceded by ".", not replaced
        assert_eq!(replace_identifier_dots("..."),    "...");
        assert_eq!(replace_identifier_dots("foo...bar"), "foo...bar");
    }

    #[test]
    fn item_numbers_spelled_digit_by_digit() {
        assert_eq!(spell_item_numbers("Item 71279"), "Item seven one two seven nine");
        assert_eq!(spell_item_numbers("Item 1000"),  "Item one zero zero zero");
        // fewer than 4 digits — left alone
        assert_eq!(spell_item_numbers("Item 42"),    "Item 42");
        // not at word boundary — left alone
        assert_eq!(spell_item_numbers("Items 71279"), "Items 71279");
    }

    #[test]
    fn ellipsis_stripped() {
        assert_eq!(replace_ellipsis("foo...bar"),        "foobar");
        assert_eq!(replace_ellipsis("foo...,bar"),       "foo,bar");
        assert_eq!(replace_ellipsis(r#"foo..." bar"#),   r#"foo" bar"#);
        assert_eq!(replace_ellipsis("foo\u{2026}bar"),   "foobar");
        assert_eq!(replace_ellipsis("foo\u{2026},bar"),  "foo,bar");
        // leading space consumed so orphaned ` ,` doesn't remain
        assert_eq!(replace_ellipsis("every ..., and"),   "every, and");
        assert_eq!(replace_ellipsis("every \u{2026}, and"), "every, and");
    }

    #[test]
    fn comma_grouped_numbers_expanded() {
        assert_eq!(expand_comma_numbers("2,000 characters"), "two thousand characters");
        assert_eq!(expand_comma_numbers("100,000 items"), "one hundred thousand items");
        // not a thousands grouping (only 2 digits after comma): "4b" is
        // letter-adjacent so stays alone; bare "12" gets digit-group spelled.
        assert_eq!(expand_comma_numbers("(4b,12)"), "(4b,twelve)");
    }

    #[test]
    fn three_digit_numbers_digit_group_spelled() {
        assert_eq!(three_digit_group_words("560"), "five sixty");
        assert_eq!(three_digit_group_words("501"), "five oh one");
        assert_eq!(three_digit_group_words("500"), "five hundred");
        assert_eq!(three_digit_group_words("100"), "one hundred");
        assert_eq!(three_digit_group_words("111"), "one eleven");
    }

    #[test]
    fn bare_three_digit_numbers_expanded_in_context() {
        assert_eq!(expand_comma_numbers("I have 560 dollars"), "I have five sixty dollars");
        assert_eq!(expand_comma_numbers("apartment 501"), "apartment five oh one");
        assert_eq!(expand_comma_numbers("500 years"), "five hundred years");
    }

    #[test]
    fn four_digit_numbers_digit_group_spelled() {
        assert_eq!(four_digit_group_words("1976"), "nineteen seventy six");
        assert_eq!(four_digit_group_words("2010"), "twenty ten");
        assert_eq!(four_digit_group_words("2005"), "twenty oh five");
        assert_eq!(four_digit_group_words("1905"), "nineteen oh five");
        assert_eq!(four_digit_group_words("1900"), "nineteen hundred");
        assert_eq!(four_digit_group_words("2300"), "twenty three hundred");
        assert_eq!(four_digit_group_words("2000"), "two thousand");
        assert_eq!(four_digit_group_words("1000"), "one thousand");
        assert_eq!(four_digit_group_words("3000"), "three thousand");
    }

    #[test]
    fn bare_four_digit_numbers_expanded_in_context() {
        assert_eq!(expand_comma_numbers("born in 1976"), "born in nineteen seventy six");
        assert_eq!(expand_comma_numbers("item 2237"), "item twenty two thirty seven");
    }

    #[test]
    fn bare_one_and_two_digit_numbers_expanded() {
        assert_eq!(expand_comma_numbers("only 12 left"), "only twelve left");
        assert_eq!(expand_comma_numbers("I had 3 cats"), "I had three cats");
        assert_eq!(expand_comma_numbers("page 0"), "page zero");
        assert_eq!(expand_comma_numbers("apartment 42"), "apartment forty two");
    }

    #[test]
    fn bare_digit_groups_excluded_when_touching_a_letter() {
        // Alphanumeric IDs aren't bare numbers — split_alphanumeric (later
        // in the pipeline) handles spacing these out, not this rule.
        assert_eq!(expand_comma_numbers("(4b,12)"), "(4b,twelve)");
        assert_eq!(expand_comma_numbers("v2 release"), "v2 release");
        assert_eq!(expand_comma_numbers("room 14B"), "room 14B");
    }

    #[test]
    fn bare_digit_groups_excluded_cases_unchanged() {
        // Leading zero (length > 1) — handled elsewhere (digit-by-digit ID
        // spelling): a lone "0" still spells out as "zero" via this rule.
        assert_eq!(expand_comma_numbers("code 012"), "code 012");
        assert_eq!(expand_comma_numbers("code 0123"), "code 0123");
        // Not 1-4 digits — untouched by this rule.
        assert_eq!(expand_comma_numbers("serial 12345"), "serial 12345");
        // Comma-grouped takes precedence over the bare-digit-group rule.
        assert_eq!(expand_comma_numbers("2,560 total"), "two thousand five hundred sixty total");
    }

    #[test]
    fn long_allcaps_title_cased() {
        assert_eq!(process_alpha_token("AUGMENT"), "Augment");
        assert_eq!(process_alpha_token("IMPORTANT"), "Important");
    }

    #[test]
    fn short_allcaps_spelled_out() {
        assert_eq!(process_alpha_token("UIS"), "U I S");
        assert_eq!(process_alpha_token("TTS"), "T T S");
        assert_eq!(process_alpha_token("DNA"), "D N A");
    }

    #[test]
    fn four_char_allcaps_title_cased() {
        assert_eq!(process_alpha_token("NASA"), "Nasa");
        assert_eq!(process_alpha_token("HTTP"), "Http");
    }

    #[test]
    fn mixed_case_untouched() {
        assert_eq!(process_alpha_token("Hello"), "Hello");
        assert_eq!(process_alpha_token("iPhone"), "iPhone");
    }

    #[test]
    fn possessive_allcaps_lowercased() {
        assert_eq!(process_alpha_token("AUGMENT's"), "Augment's");
        assert_eq!(process_alpha_token("TTS's"), "T T S's");
    }

    #[test]
    fn alphanumeric_split() {
        // Digit runs are spelled out, not left raw — raw digits are hard
        // for TTS and untimeable by forced alignment (see split_alphanumeric).
        assert_eq!(split_alphanumeric("1a"),    "one a");
        assert_eq!(split_alphanumeric("4c2"),   "four c two");
        assert_eq!(split_alphanumeric("4C2"),   "four C two");
        assert_eq!(split_alphanumeric("12ab34"), "twelve ab thirty four");
        // Memory-size shorthand: the motivating case.
        assert_eq!(split_alphanumeric("512K"),  "five twelve K");
        assert_eq!(split_alphanumeric("128K"),  "one twenty eight K");
        // Longer/leading-zero digit runs fall back to digit-by-digit.
        assert_eq!(split_alphanumeric("007b"),  "zero zero seven b");
    }

    #[test]
    fn tag_expansion() {
        assert_eq!(try_expand_tag("Ref-3"),   Some("Ref three".into()));
        assert_eq!(try_expand_tag("Ref-12"),  Some("Ref twelve".into()));
        assert_eq!(try_expand_tag("Fig-1"),   Some("Figure one".into()));
        assert_eq!(try_expand_tag("Table-2"), Some("Table two".into()));
        assert_eq!(try_expand_tag("em"),      None);
        assert_eq!(try_expand_tag("Foo-3"),   Some("Foo three".into()));
        // 4-digit tag uses the same digit-group convention as Pass 7/4.
        assert_eq!(try_expand_tag("Ref-1976"), Some("Ref nineteen seventy six".into()));
        // 5+ digits aren't covered by `spell_bare_digit_group` — left bare.
        assert_eq!(try_expand_tag("Ref-12345"), Some("Ref 12345".into()));
    }

    #[test]
    fn year_range_expansion() {
        assert_eq!(expand_year_ranges("1976-77"),
            "nineteen seventy six to seventy seven");
        assert_eq!(expand_year_ranges("1976-1977"),
            "nineteen seventy six to nineteen seventy seven");
        assert_eq!(expand_year_ranges("in 1976-77 we"),
            "in nineteen seventy six to seventy seven we");
        assert_eq!(expand_year_ranges("pages 10-20"),
            "pages ten to twenty");
        // Regression: both halves must be spelled, not just the hyphen
        // replaced — bare digits here would be invisible to forced
        // alignment (no letters), dropping an annotation spanning them
        // entirely from the aligned word list rather than just mismatching.
        assert_eq!(expand_year_ranges("in Winchester, MA in 1973-76"),
            "in Winchester, MA in nineteen seventy three to seventy six");
    }

    #[test]
    fn hyphen_becomes_space() {
        assert_eq!(normalize("one-handed"), "one handed");
        assert_eq!(normalize("on-line"),    "on line");
    }

    #[test]
    fn full_citation_sentence() {
        let input = "as reported in <Ref-3> and <Ref-4>";
        assert_eq!(normalize(input), "as reported in Ref three and Ref four");
    }

    #[test]
    fn em_dash_becomes_comma() {
        assert_eq!(
            normalize("Statement 4b -- or, to conceptualize"),
            "Statement four b, or, to conceptualize"
        );
    }

    #[test]
    fn acronym_without_override_spells_out() {
        let overrides = HashMap::new();
        assert_eq!(process_token("SID", &overrides), "S I D");
    }

    #[test]
    fn journal_links_expanded() {
        assert_eq!(expand_journal_links("(AUGMENT,71279,)"), "Augment seven one two seven nine");
        assert_eq!(expand_journal_links("<OAD,2237,>"),      "O A D two two three seven");
        assert_eq!(expand_journal_links("(AUGMENT,14724,)"), "Augment one four seven two four");
        // lowercase name — not matched (DDD,xxx,bb style placeholders)
        assert_eq!(expand_journal_links("(DDD,xxx,bb)"),     "(DDD,xxx,bb)");
        // digits in second field required — not matched if non-digit
        assert_eq!(expand_journal_links("(DDD,xxx,)"),       "(DDD,xxx,)");
    }

    #[test]
    fn brackets_stripped() {
        assert_eq!(normalize("<Ref-8>.) and (EEE,yy,cc)."), "Ref eight. and E E E,yy,cc.");
    }

    #[test]
    fn colon_override_applied() {
        // <4b:mi> is defined in tts_overrides.txt; the override key
        // contains ':' not '.', which previously wasn't matched by
        // apply_punctuated_overrides.
        // Pass 2 strips the surrounding quotes (1 word ≤ 5).
        assert_eq!(normalize("\"<4b:mi>\""), "4 b colon M I");
    }

    #[test]
    fn override_forces_acronym_to_word() {
        // A 3-letter acronym that should be pronounced as a word, not spelled out.
        let mut overrides = HashMap::new();
        overrides.insert("sql".to_string(), "sequel".to_string());
        assert_eq!(process_token("SQL", &overrides), "sequel");
    }

    #[test]
    fn override_forces_long_acronym_to_spell_out() {
        // A >3-letter acronym that would normally be lowercased, but should
        // be spelled out letter-by-letter instead.
        let mut overrides = HashMap::new();
        overrides.insert("nasa".to_string(), "N A S A".to_string());
        assert_eq!(process_token("NASA", &overrides), "N A S A");
    }

    #[test]
    fn state_abbrevs_expanded() {
        assert_eq!(expand_state_abbrevs("Denver, CO"), "Denver, Colorado");
        assert_eq!(expand_state_abbrevs("Austin, TX and Albany, NY"),
                   "Austin, Texas and Albany, New York");
    }

    #[test]
    fn state_abbrevs_unknown_code_left_unchanged() {
        // "ZZ" isn't a postal code — leave the comma-led pattern as-is.
        assert_eq!(expand_state_abbrevs("Nowhere, ZZ"), "Nowhere, ZZ");
    }

    #[test]
    fn state_abbrevs_only_fire_after_comma() {
        // Not preceded by ", " — must not be treated as a state code.
        assert_eq!(expand_state_abbrevs("the CO task"), "the CO task");
        // Word boundary: a longer all-caps run starting with a valid code
        // must not be truncated/matched.
        assert_eq!(expand_state_abbrevs("score, COOL"), "score, COOL");
    }

    #[test]
    fn tag_expansion_fallback_unchanged() {
        // Lowercase tag word — not a citation/figure/table marker, left as-is.
        assert_eq!(expand_tags("see <em> note"), "see <em> note");
        // Uppercase word but non-digit suffix — rejected by try_expand_tag,
        // tag is re-emitted literally rather than dropped.
        assert_eq!(expand_tags("see <Ref-x> note"), "see <Ref-x> note");
    }

    #[test]
    fn year_range_not_expanded_when_followed_by_letter() {
        // Trailing letter right after the second digit run means this isn't
        // a bare year range (e.g. a version/identifier) — leave untouched.
        assert_eq!(expand_year_ranges("1976-77a"), "1976-77a");
    }

    #[test]
    fn parse_overrides_warns_on_missing_tab() {
        // No tab delimiter — line is ignored (warned, not inserted), rest
        // of the file still parses normally.
        let map = parse_overrides("noTabHere\nfoo\tbar\n");
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("foo"), Some(&"bar".to_string()));
    }

    #[test]
    fn digit_word_covers_all_digits_and_fallback() {
        assert_eq!(digit_word('0'), "zero");
        assert_eq!(digit_word('9'), "nine");
        assert_eq!(digit_word('x'), "");
    }

    #[test]
    fn add_override_in_inserts_lowercased_key() {
        let mut map = HashMap::new();
        add_override_in(&mut map, "SQL", "sequel");
        assert_eq!(map.get("sql"), Some(&"sequel".to_string()));
    }

    #[test]
    fn remove_override_in_removes_existing_and_reports_absence() {
        let mut map = HashMap::new();
        map.insert("sql".to_string(), "sequel".to_string());
        assert!(remove_override_in(&mut map, "SQL"));
        assert!(map.is_empty());
        assert!(!remove_override_in(&mut map, "sql"));
    }

    #[test]
    fn list_overrides_in_sorted_by_word() {
        let mut map = HashMap::new();
        map.insert("zebra".to_string(), "z".to_string());
        map.insert("apple".to_string(), "a".to_string());
        let pairs = list_overrides_in(&map);
        assert_eq!(pairs, vec![
            ("apple".to_string(), "a".to_string()),
            ("zebra".to_string(), "z".to_string()),
        ]);
    }

    #[test]
    fn save_map_in_round_trips_through_parse_overrides() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("tts_overrides.txt");

        let mut map = HashMap::new();
        map.insert("sql".to_string(), "sequel".to_string());
        map.insert("e.g.".to_string(), "for example".to_string());
        save_map_in(&path, &map);

        let contents = std::fs::read_to_string(&path).unwrap();
        let reloaded = parse_overrides(&contents);
        assert_eq!(reloaded, map);
    }

    // -----------------------------------------------------------------------
    // Span-mapping proof of concept
    // -----------------------------------------------------------------------

    #[test]
    fn spanned_year_range_matches_unspanned_text() {
        for input in ["1976-77", "in 1976-77 we", "pages 10-20", "no years here"] {
            let chunks = expand_year_ranges_spanned(spanned_from_input(input));
            assert_eq!(flatten(&chunks), expand_year_ranges(input));
        }
    }

    #[test]
    fn spanned_year_range_source_span_covers_original_digits() {
        let input = "in 1976-77 we";
        let chunks = expand_year_ranges_spanned(spanned_from_input(input));
        let normalized = flatten(&chunks);
        assert_eq!(normalized, "in nineteen seventy six to seventy seven we");

        // The expanded chunk should map back to "1976-77" in the original
        // input (chars 3..10).
        let offset_in_expansion = normalized.find("to").unwrap();
        assert_eq!(source_span_at(&chunks, offset_in_expansion), Some(3..10));

        // A char before the expansion falls in the leading "in " chunk,
        // which maps back to the whole unprocessed "in " run (0..3) — per
        // the chunk-granularity design, not a single character.
        assert_eq!(source_span_at(&chunks, 0), Some(0..3));
    }

    #[test]
    fn spanned_year_range_preserves_unexpanded_text_spans() {
        // No expansion happens — single passthrough chunk spanning the whole input.
        let input = "no years here";
        let chunks = expand_year_ranges_spanned(spanned_from_input(input));
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].src, 0..input.chars().count());
    }

    #[test]
    fn spanned_hyphens_to_spaces_matches_unspanned_text() {
        for input in ["one-handed", "on-line", "no hyphens"] {
            let chunks = hyphens_to_spaces_spanned(spanned_from_input(input));
            assert_eq!(flatten(&chunks), input.replace('-', " "));
        }
    }

    #[test]
    fn spanned_hyphens_to_spaces_preserves_1to1_span() {
        let chunks = hyphens_to_spaces_spanned(spanned_from_input("on-line"));
        // Trivial 1:1 transform — chunk span is unchanged from the input.
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].src, 0.."on-line".chars().count());
        assert_eq!(chunks[0].text, "on line");
    }

    #[test]
    fn spanned_passes_compose_across_pipeline() {
        // Chain two passes — confirms spans stay anchored to the *original*
        // input (not the intermediate output) when threaded through more
        // than one transform.
        let input = "the 1976-77 report-card";
        let chunks = spanned_from_input(input);
        let chunks = expand_year_ranges_spanned(chunks);
        let chunks = hyphens_to_spaces_spanned(chunks);

        let normalized = flatten(&chunks);
        assert_eq!(normalized, "the nineteen seventy six to seventy seven report card");

        // "to" still maps back to "1976-77" in the *original* input, even
        // though a later pass (hyphen removal) also ran on this text.
        let offset_in_expansion = normalized.find("to").unwrap();
        assert_eq!(source_span_at(&chunks, offset_in_expansion), Some(4..11));
    }

    #[test]
    fn spanned_strip_brackets_matches_unspanned() {
        let input = "<Ref-8>.) and (EEE,yy,cc).";
        let chunks = strip_brackets_spanned(spanned_from_input(input));
        let expected: String = input.chars().filter(|c| !matches!(c, '(' | ')' | '<' | '>' | '[' | ']')).collect();
        assert_eq!(flatten(&chunks), expected);
    }

    #[test]
    fn spanned_strip_brackets_preserves_spans_around_removed_chars() {
        // Bracket removal shrinks text — confirms surrounding text stays
        // correctly anchored (not silently made non-raw/unscannable) once
        // a bracket char is removed from the middle of it.
        let input = "see (Augment) now";
        let chunks = strip_brackets_spanned(spanned_from_input(input));
        let normalized = flatten(&chunks);
        assert_eq!(normalized, "see Augment now");
        let offset = normalized.find("now").unwrap();
        // The chunk containing "now" also includes the space right after
        // the removed ")", since nothing split them apart — per
        // chunk-level granularity, the lookup returns that whole chunk's
        // source range (the space at index 13 through "now" ending at 17).
        assert_eq!(source_span_at(&chunks, offset), Some(13..17));
    }

    #[test]
    fn spanned_ellipsis_matches_unspanned() {
        for input in ["foo...bar", "every ..., and", "no ellipsis here"] {
            let chunks = replace_ellipsis_spanned(spanned_from_input(input));
            assert_eq!(flatten(&chunks), replace_ellipsis(input));
        }
    }

    #[test]
    fn spanned_identifier_dots_matches_unspanned() {
        for input in ["4b.l", "Ref.dt", "end.", "..."] {
            let chunks = replace_identifier_dots_spanned(spanned_from_input(input));
            assert_eq!(flatten(&chunks), replace_identifier_dots(input));
        }
    }

    #[test]
    fn spanned_identifier_dots_source_span_is_single_char() {
        let input = "4b.l";
        let chunks = replace_identifier_dots_spanned(spanned_from_input(input));
        let normalized = flatten(&chunks);
        assert_eq!(normalized, "4b dot l");
        let offset_of_dot = normalized.find("dot").unwrap();
        // The "." was a single char at index 2 in the original input.
        assert_eq!(source_span_at(&chunks, offset_of_dot), Some(2..3));
    }

    #[test]
    fn spanned_comma_numbers_matches_unspanned() {
        for input in ["2,000 characters", "100,000 items", "(4b,12)", "no numbers"] {
            let chunks = expand_comma_numbers_spanned(spanned_from_input(input));
            assert_eq!(flatten(&chunks), expand_comma_numbers(input));
        }
    }

    #[test]
    fn spanned_comma_numbers_source_span_covers_digit_run() {
        let input = "I have 2,000 apples";
        let chunks = expand_comma_numbers_spanned(spanned_from_input(input));
        let normalized = flatten(&chunks);
        assert_eq!(normalized, "I have two thousand apples");
        let offset = normalized.find("two").unwrap();
        // "2,000" spans chars 7..12 in the original input.
        assert_eq!(source_span_at(&chunks, offset), Some(7..12));
    }

    #[test]
    fn spanned_item_numbers_matches_unspanned() {
        for input in ["Item 71279", "Item 1000", "Item 42", "Items 71279"] {
            let chunks = spell_item_numbers_spanned(spanned_from_input(input));
            assert_eq!(flatten(&chunks), spell_item_numbers(input));
        }
    }

    #[test]
    fn spanned_item_numbers_source_span_covers_whole_match() {
        let input = "see Item 71279 now";
        let chunks = spell_item_numbers_spanned(spanned_from_input(input));
        let normalized = flatten(&chunks);
        assert_eq!(normalized, "see Item seven one two seven nine now");
        let offset = normalized.find("seven one").unwrap();
        // "Item 71279" spans chars 4..14 in the original input.
        assert_eq!(source_span_at(&chunks, offset), Some(4..14));
    }

    #[test]
    fn spanned_journal_links_matches_unspanned() {
        for input in ["(AUGMENT,71279,)", "<OAD,2237,>", "(DDD,xxx,bb)", "(DDD,xxx,)", "no links"] {
            let chunks = expand_journal_links_spanned(spanned_from_input(input));
            assert_eq!(flatten(&chunks), expand_journal_links(input));
        }
    }

    #[test]
    fn spanned_journal_links_source_span_covers_whole_link() {
        let input = "see (AUGMENT,71279,) now";
        let chunks = expand_journal_links_spanned(spanned_from_input(input));
        let normalized = flatten(&chunks);
        assert_eq!(normalized, "see Augment seven one two seven nine now");
        let offset = normalized.find("Augment").unwrap();
        // "(AUGMENT,71279,)" spans chars 4..20 in the original input.
        assert_eq!(source_span_at(&chunks, offset), Some(4..20));
    }

    #[test]
    fn spanned_state_abbrevs_matches_unspanned() {
        for input in ["Denver, CO", "Austin, TX and Albany, NY", "Nowhere, ZZ", "the CO task", "score, COOL"] {
            let chunks = expand_state_abbrevs_spanned(spanned_from_input(input));
            assert_eq!(flatten(&chunks), expand_state_abbrevs(input));
        }
    }

    #[test]
    fn spanned_state_abbrevs_source_span_is_four_chars() {
        let input = "live in Denver, CO";
        let chunks = expand_state_abbrevs_spanned(spanned_from_input(input));
        let normalized = flatten(&chunks);
        assert_eq!(normalized, "live in Denver, Colorado");
        let offset = normalized.find("Colorado").unwrap();
        // ", CO" spans chars 14..18 in the original input.
        assert_eq!(source_span_at(&chunks, offset), Some(14..18));
    }

    #[test]
    fn spanned_em_dash_matches_unspanned() {
        for input in ["Statement 4b -- or, to conceptualize", "lines \u{2014} things", "no dashes"] {
            let chunks = replace_em_dash_and_double_hyphen_spanned(spanned_from_input(input));
            assert_eq!(flatten(&chunks), replace_em_dash_and_double_hyphen(input));
        }
    }

    #[test]
    fn spanned_tokenize_matches_unspanned() {
        let overrides = HashMap::new();
        for input in ["one-handed", "UIS spoke", "AUGMENT's report", "0609 was the code"] {
            let chunks = tokenize_spanned(spanned_from_input(input), &overrides);
            assert_eq!(flatten(&chunks), tokenize(input, &overrides));
        }
    }

    #[test]
    fn spanned_tokenize_source_span_covers_whole_token() {
        let overrides = HashMap::new();
        let input = "see AUGMENT's report";
        let chunks = tokenize_spanned(spanned_from_input(input), &overrides);
        let normalized = flatten(&chunks);
        assert_eq!(normalized, "see Augment's report");
        let offset = normalized.find("Augment's").unwrap();
        // "AUGMENT's" spans chars 4..13 in the original input.
        assert_eq!(source_span_at(&chunks, offset), Some(4..13));
    }

    #[test]
    fn spanned_punctuated_overrides_matches_unspanned() {
        let mut overrides = HashMap::new();
        overrides.insert("e.g.".to_string(), "for example".to_string());
        for input in ["see e.g. this", "no override here"] {
            let chunks = apply_punctuated_overrides_spanned(spanned_from_input(input), &overrides);
            assert_eq!(flatten(&chunks), apply_punctuated_overrides(input, &overrides));
        }
    }

    #[test]
    fn spanned_punctuated_overrides_source_span_covers_matched_key() {
        let mut overrides = HashMap::new();
        overrides.insert("e.g.".to_string(), "for example".to_string());
        let input = "see e.g. this";
        let chunks = apply_punctuated_overrides_spanned(spanned_from_input(input), &overrides);
        let normalized = flatten(&chunks);
        assert_eq!(normalized, "see for example this");
        let offset = normalized.find("for example").unwrap();
        // "e.g." spans chars 4..8 in the original input.
        assert_eq!(source_span_at(&chunks, offset), Some(4..8));
    }

    #[test]
    fn spanned_full_pipeline_matches_normalize() {
        // Run every converted pass in the same order as `normalize()` and
        // confirm the flattened result matches exactly — the real proof
        // that the chunk-threading approach holds up end-to-end, not just
        // pass-by-pass.
        let overrides = HashMap::new();
        for input in [
            "Item 71279 was reported in 1976-77, see (AUGMENT,71279,) near Denver, CO -- relevant.",
            "as reported in <Ref-3> and <Ref-4>",
            "one-handed on-line 0609",
            "no special handling needed here",
        ] {
            let chunks = spanned_from_input(input);
            let chunks = apply_punctuated_overrides_spanned(chunks, &overrides);
            let chunks = expand_tags_spanned(chunks);
            let chunks = expand_year_ranges_spanned(chunks);
            let chunks = replace_em_dash_and_double_hyphen_spanned(chunks);
            let chunks = spell_item_numbers_spanned(chunks);
            let chunks = expand_comma_numbers_spanned(chunks);
            let chunks = expand_journal_links_spanned(chunks);
            let chunks = expand_state_abbrevs_spanned(chunks);
            let chunks = replace_identifier_dots_spanned(chunks);
            let chunks = tokenize_spanned(chunks, &overrides);
            let chunks = strip_brackets_spanned(chunks);
            let chunks = replace_ellipsis_spanned(chunks);

            let expected_text = apply_punctuated_overrides(input, &overrides);
            let expected_text = strip_short_quotes(&expected_text);
            let expected_text = expand_tags(&expected_text);
            let expected_text = expand_year_ranges(&expected_text);
            let expected_text = replace_em_dash_and_double_hyphen(&expected_text);
            let expected_text = spell_item_numbers(&expected_text);
            let expected_text = expand_comma_numbers(&expected_text);
            let expected_text = expand_journal_links(&expected_text);
            let expected_text = expand_state_abbrevs(&expected_text);
            let expected_text = replace_identifier_dots(&expected_text);
            let expected_text = tokenize(&expected_text, &overrides);
            let expected_text: String = expected_text.chars()
                .filter(|c| !matches!(c, '(' | ')' | '<' | '>' | '[' | ']')).collect();
            let expected_text = replace_ellipsis(&expected_text);

            assert_eq!(flatten(&chunks), expected_text, "mismatch for input: {input:?}");
        }
    }

    #[test]
    fn spanned_full_pipeline_source_span_for_item_number() {
        // End-to-end span check: the annotation-matching use case — find
        // where an expanded word lands in the normalized text, then confirm
        // it maps back to the right span in the *original* sentence.
        let overrides = HashMap::new();
        let input = "see Item 71279 in the report";

        let chunks = spanned_from_input(input);
        let chunks = apply_punctuated_overrides_spanned(chunks, &overrides);
        let chunks = expand_tags_spanned(chunks);
        let chunks = expand_year_ranges_spanned(chunks);
        let chunks = replace_em_dash_and_double_hyphen_spanned(chunks);
        let chunks = expand_comma_numbers_spanned(chunks);
        let chunks = spell_item_numbers_spanned(chunks);
        let chunks = expand_journal_links_spanned(chunks);
        let chunks = expand_state_abbrevs_spanned(chunks);
        let chunks = replace_identifier_dots_spanned(chunks);
        let chunks = tokenize_spanned(chunks, &overrides);
        let chunks = strip_brackets_spanned(chunks);
        let chunks = replace_ellipsis_spanned(chunks);

        let normalized = flatten(&chunks);
        assert_eq!(normalized, "see Item seven one two seven nine in the report");

        let offset = normalized.find("seven one").unwrap();
        // "Item 71279" spans chars 4..14 in the original input.
        assert_eq!(source_span_at(&chunks, offset), Some(4..14));
    }

    #[test]
    fn normalize_with_spans_matches_normalize_text() {
        for input in [
            "Item 71279 was reported in 1976-77, see (AUGMENT,71279,) near Denver, CO -- relevant.",
            "\"<4b:mi>\"",
            "as reported in <Ref-3> and <Ref-4>",
        ] {
            assert_eq!(normalize_with_spans(input).text, normalize(input));
        }
    }

    #[test]
    fn normalize_with_spans_source_range_for_item_number() {
        let input = "see Item 71279 in the report";
        let result = normalize_with_spans(input);
        assert_eq!(result.text, "see Item seven one two seven nine in the report");

        let offset = result.text.find("seven one").unwrap();
        let match_len = "seven one two seven nine".len();
        let source = result.source_range(offset..offset + match_len);

        // "Item 71279" spans chars 4..14 in the original input.
        assert_eq!(source, Some(4..14));
        assert_eq!(&input[4..14], "Item 71279");
    }

    #[test]
    fn normalize_with_spans_source_range_out_of_bounds_is_none() {
        let result = normalize_with_spans("short");
        assert_eq!(result.source_range(0..1000), None);
        // Empty/inverted range is also rejected.
        assert_eq!(result.source_range(2..2), None);
    }
}
