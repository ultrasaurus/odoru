/// normalizer.rs — text normalization applied before TTS synthesis.
///
/// Processing order:
///   0. Strip short inline quotes (≤5 words): "foo bar" → foo bar.
///   1. Expand known `<Tag-N>` citation/figure/table markers.
///   2. Expand year ranges: 1976-77 → "1976 to 77".
///   2b. Expand comma-grouped numbers: 2,000 → "two thousand".
///   2c. Spell item/reference numbers digit-by-digit: Item 71279 → Item seven one …
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

/// Save the current in-memory map back to `tts_overrides.txt`.
fn save_map(map: &HashMap<String, String>) {
    let mut lines: Vec<String> = map.iter()
        .map(|(k, v)| format!("{k}\t{v}"))
        .collect();
    lines.sort();
    let contents = lines.join("\n") + "\n";
    if let Err(e) = std::fs::write(&state().path, &contents) {
        error!("failed to write tts_overrides.txt: {e}");
    }
}

// ---------------------------------------------------------------------------
// Public override management API
// ---------------------------------------------------------------------------

/// Add or update a pronunciation override and persist to disk.
pub fn add_override(word: &str, replacement: &str) {
    let mut map = state().map.write().expect("overrides lock poisoned");
    map.insert(word.to_lowercase(), replacement.to_owned());
    save_map(&map);
}

/// Remove a pronunciation override and persist to disk. Returns true if it existed.
pub fn remove_override(word: &str) -> bool {
    let mut map = state().map.write().expect("overrides lock poisoned");
    let existed = map.remove(&word.to_lowercase()).is_some();
    if existed { save_map(&map); }
    existed
}

/// Return all current overrides as a sorted vec of (word, replacement) pairs.
pub fn list_overrides() -> Vec<(String, String)> {
    let map = read_map();
    let mut pairs: Vec<_> = map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    pairs
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Normalize `text` for TTS pronunciation.
pub fn normalize(text: &str) -> String {
    let overrides = read_map();

    // Pass 3: punctuated overrides run first — before quote-stripping — so
    // keys like `"."` and `"*D"` can match before their quotes are removed.
    let text = apply_punctuated_overrides(text, &*overrides);

    // Pass 0: strip short inline quotes (≤5 words) — prevents TTS mangling
    // of brief quoted phrases by removing the quotation marks.
    let text = strip_short_quotes(&text);

    // Pass 1: expand <Tag-N> markers.
    let text = expand_tags(&text);

    // Pass 2: expand year ranges (4-digit year, hyphen, 2+ digits).
    let text = expand_year_ranges(&text);

    // Pass 2a: replace em-dashes and double-hyphens with comma so TTS gets a
    // clean pause cue instead of extra whitespace that causes vocalisation artifacts.
    let text = text.replace('\u{2014}', ",").replace(" -- ", ", ");

    // Pass 2b: expand comma-grouped numbers (2,000 -> "two thousand").
    let text = expand_comma_numbers(&text);

    // Pass 2c: spell Item/reference numbers digit-by-digit (Item 71279 →
    // Item seven one two seven nine) so TTS doesn't garble large IDs.
    let text = spell_item_numbers(&text);

    // Pass 3b: replace dots between alphanumeric chars with " dot " so link
    // notation like `4b.l` or `Ref.dt` is read correctly.
    let text = replace_identifier_dots(&text);

    // Pass 4: tokenize and process word/alphanumeric tokens.
    let mut out = String::with_capacity(text.len());
    let mut tok_buf = String::new();

    let flush_token = |buf: &mut String, out: &mut String| {
        if buf.is_empty() { return; }
        out.push_str(&process_token(buf, &*overrides));
        buf.clear();
    };

    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '\'' {
            tok_buf.push(ch);
        } else {
            flush_token(&mut tok_buf, &mut out);
            // Pass 5: replace remaining hyphens with spaces.
            out.push(if ch == '-' { ' ' } else { ch });
        }
    }
    flush_token(&mut tok_buf, &mut out);

    // Pass 6: strip bracket characters (keeping their contents) — VibeVoice
    // hallucinates on tokens with `<>`/`()`/`[]` next to other punctuation.
    let out: String = out.chars().filter(|c| !matches!(c, '(' | ')' | '<' | '>' | '[' | ']')).collect();

    // Pass 7: replace ellipses with newlines so VibeVoice treats the pause
    // as a sentence boundary rather than looping on an unfinished sentence.
    replace_ellipsis(&out)
}

// ---------------------------------------------------------------------------
// Pass 1: <Tag-N> expansion
// ---------------------------------------------------------------------------

fn expand_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '<' {
            let start = i;
            i += 1;
            let mut tag = String::new();
            while i < chars.len() && chars[i] != '>' && chars[i] != '<' {
                tag.push(chars[i]);
                i += 1;
            }
            if i < chars.len() && chars[i] == '>' {
                i += 1;
                if let Some(expanded) = try_expand_tag(&tag) {
                    out.push_str(&expanded);
                    continue;
                }
            }
            out.push('<');
            out.push_str(&tag);
            if i > start && chars[i - 1] == '>' { out.push('>'); }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
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
    Some(format!("{} {}", spoken, digits))
}

// ---------------------------------------------------------------------------
// Pass 2: year range expansion
// ---------------------------------------------------------------------------

fn expand_year_ranges(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;

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
                        for &c in &chars[start..i] { out.push(c); }
                        out.push_str(" to ");
                        for &c in &chars[after_hyphen..end] { out.push(c); }
                        i = end;
                        continue;
                    }
                }
            }
            for &c in &chars[start..i] { out.push(c); }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Pass 2b: comma-grouped number expansion
// ---------------------------------------------------------------------------

/// Expand comma-grouped numbers like "2,000" or "100,000" into words
/// ("two thousand", "one hundred thousand"). Requires each group after the
/// first comma to have exactly 3 digits, and the whole number not to be
/// adjacent to other digits/letters (so "Ref-1,000" style codes are left
/// alone).
fn expand_comma_numbers(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;

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
                    out.push_str(&number_to_words(n));
                    i = end;
                    continue;
                }
            }
            for &c in &chars[start..j] { out.push(c); }
            i = j;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Pass 3: handle punctuated overrides before token processing, so they can
// match across punctuation (e.g. `e.g.`).
// ---------------------------------------------------------------------------

fn apply_punctuated_overrides(text: &str, overrides: &HashMap<String, String>) -> String {
    let mut text = text.to_owned();
    // A "punctuated" override key contains any character that pass 4's
    // tokenizer would otherwise split on.
    for (from, to) in overrides.iter().filter(|(k, _)| !k.chars().all(|c| c.is_alphanumeric() || c == '\'')) {
        let lower = text.to_lowercase();
        let mut result = String::with_capacity(text.len());
        let mut pos = 0;
        while pos < lower.len() {
            if lower[pos..].starts_with(from.as_str()) {
                result.push_str(to);
                pos += from.len();
            } else {
                let ch = text[pos..].chars().next().unwrap();
                result.push(ch);
                pos += ch.len_utf8();
            }
        }
        text = result;
    }
    text
}

// ---------------------------------------------------------------------------
// Pass 4: token processing
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

fn split_alphanumeric(token: &str) -> String {
    let mut out = String::new();
    let mut prev_kind: Option<bool> = None;

    for ch in token.chars() {
        let is_digit = ch.is_ascii_digit();
        if let Some(prev) = prev_kind {
            if prev != is_digit { out.push(' '); }
        }
        out.push(ch);
        prev_kind = Some(is_digit);
    }
    out
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
            // Title-case: capitalize first letter so "INTRODUCTION" → "Introduction"
            // rather than "introduction". Sounds the same to TTS but reads more
            // naturally and avoids issues when preceded by punctuation (e.g. "I. Introduction").
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
// Pass 0: strip short inline quotes
// ---------------------------------------------------------------------------

fn strip_short_quotes(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '"' {
            let start = i + 1;
            let mut j = start;
            while j < chars.len() && chars[j] != '"' { j += 1; }
            if j < chars.len() {
                let content: String = chars[start..j].iter().collect();
                if content.split_whitespace().count() <= 5 {
                    out.push_str(&content);
                    i = j + 1;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// Pass 2c: spell Item/reference numbers digit-by-digit
// ---------------------------------------------------------------------------

fn spell_item_numbers(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
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
                out.push_str("Item ");
                for (k, &c) in chars[digit_start..j].iter().enumerate() {
                    if k > 0 { out.push(' '); }
                    out.push_str(digit_word(c));
                }
                i = j;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
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
// Pass 3b: replace dots between alphanumeric chars with " dot "
// ---------------------------------------------------------------------------

fn replace_identifier_dots(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 16);
    for i in 0..chars.len() {
        // Replace "." with " dot " when followed by alphanumeric and not part
        // of "..." (i.e. not preceded by another "."). Catches both `4b.l`
        // (alphanumeric before) and standalone ` .l ` (space before).
        if chars[i] == '.'
            && (i == 0 || chars[i - 1] != '.')
            && i + 1 < chars.len() && chars[i + 1].is_alphanumeric()
        {
            out.push_str(" dot ");
        } else {
            out.push(chars[i]);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Pass 7: replace ellipses with newlines
// ---------------------------------------------------------------------------

fn replace_ellipsis(text: &str) -> String {
    // Strip ellipses entirely — "..." tricks VibeVoice into thinking the
    // sentence is unfinished, causing looping/echoing artifacts. Adding a
    // newline or period in their place causes a "taunt" mispronunciation.
    // Also consume any leading whitespace so `every ...,` → `every,` rather
    // than leaving an orphaned ` ,` that TTS mispronounces.
    let text = text.replace(" ...", "").replace(" \u{2026}", "");
    text.replace("...", "").replace('\u{2026}', "")
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
        // not a thousands grouping (only 2 digits after comma) — left alone.
        assert_eq!(expand_comma_numbers("(4b,12)"), "(4b,12)");
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
        assert_eq!(split_alphanumeric("1a"),    "1 a");
        assert_eq!(split_alphanumeric("4c2"),   "4 c 2");
        assert_eq!(split_alphanumeric("4C2"),   "4 C 2");
        assert_eq!(split_alphanumeric("12ab34"), "12 ab 34");
    }

    #[test]
    fn tag_expansion() {
        assert_eq!(try_expand_tag("Ref-3"),   Some("Ref 3".into()));
        assert_eq!(try_expand_tag("Ref-12"),  Some("Ref 12".into()));
        assert_eq!(try_expand_tag("Fig-1"),   Some("Figure 1".into()));
        assert_eq!(try_expand_tag("Table-2"), Some("Table 2".into()));
        assert_eq!(try_expand_tag("em"),      None);
        assert_eq!(try_expand_tag("Foo-3"),   Some("Foo 3".into()));
    }

    #[test]
    fn year_range_expansion() {
        assert_eq!(expand_year_ranges("1976-77"),        "1976 to 77");
        assert_eq!(expand_year_ranges("1976-1977"),      "1976 to 1977");
        assert_eq!(expand_year_ranges("in 1976-77 we"),  "in 1976 to 77 we");
        assert_eq!(expand_year_ranges("pages 10-20"),    "pages 10 to 20");
    }

    #[test]
    fn hyphen_becomes_space() {
        assert_eq!(normalize("one-handed"), "one handed");
        assert_eq!(normalize("on-line"),    "on line");
    }

    #[test]
    fn full_citation_sentence() {
        let input = "as reported in <Ref-3> and <Ref-4>";
        assert_eq!(normalize(input), "as reported in Ref 3 and Ref 4");
    }

    #[test]
    fn em_dash_becomes_comma() {
        assert_eq!(
            normalize("Statement 4b -- or, to conceptualize"),
            "Statement 4 b, or, to conceptualize"
        );
    }

    #[test]
    fn acronym_without_override_spells_out() {
        let overrides = HashMap::new();
        assert_eq!(process_token("SID", &overrides), "S I D");
    }

    #[test]
    fn brackets_stripped() {
        assert_eq!(normalize("<Ref-8>.) and (EEE,yy,cc)."), "Ref 8. and E E E,yy,cc.");
    }

    #[test]
    fn colon_override_applied() {
        // <4b:mi> is defined in tts_overrides.txt; the override key
        // contains ':' not '.', which previously wasn't matched by
        // apply_punctuated_overrides.
        // Pass 0 strips the surrounding quotes (1 word ≤ 5).
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
}
