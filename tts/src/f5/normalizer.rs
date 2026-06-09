/// normalizer.rs — text normalization applied before TTS synthesis.
///
/// Processing order:
///   1. Expand known `<Tag-N>` citation/figure/table markers.
///   2. Expand year ranges: 1976-77 → "1976 to 77".
///   3. Load `tts_overrides.txt` and apply punctuated overrides (e.g. "e.g.").
///   4. Tokenize on word boundaries:
///      a. Apply single-word overrides (case-insensitive).
///      b. Spell out short all-caps (≤3 chars) letter by letter: UIS → U I S.
///      c. Lowercase long all-caps (>3 chars): AUGMENT → augment.
///      d. Insert spaces in alphanumeric tokens: 1a → 1 a, 4c2 → 4 c 2.
///   5. Replace remaining hyphens with spaces.
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
        if line.is_empty() || line.starts_with('#') { continue; }
        let mut cols = line.splitn(2, |c: char| c.is_whitespace());
        if let (Some(from), Some(to)) = (cols.next(), cols.next()) {
            let to = to.trim().split('#').next().unwrap_or("").trim();
            if !to.is_empty() {
                map.insert(from.to_lowercase(), to.to_owned());
            }
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

    // Pass 1: expand <Tag-N> markers.
    let text = expand_tags(text);

    // Pass 2: expand year ranges (4-digit year, hyphen, 2+ digits).
    let text = expand_year_ranges(&text);

    // Pass 3: apply punctuated overrides (those containing '.' or '-').
    let text = apply_punctuated_overrides(&text, &*overrides);

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

    out
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
// Pass 3: punctuated overrides
// ---------------------------------------------------------------------------

fn apply_punctuated_overrides(text: &str, overrides: &HashMap<String, String>) -> String {
    let mut text = text.to_owned();
    for (from, to) in overrides.iter().filter(|(k, _)| k.contains('.')) {
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

    if all_caps {
        if alpha_count <= 3 {
            let spelled: Vec<String> = stem.chars().map(|c| c.to_string()).collect();
            let result = spelled.join(" ");
            if suffix.is_empty() { result } else { format!("{}{}", result, suffix.to_lowercase()) }
        } else {
            format!("{}{}", stem.to_lowercase(), suffix.to_lowercase())
        }
    } else {
        token.to_owned()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_allcaps_lowercased() {
        assert_eq!(process_alpha_token("AUGMENT"), "augment");
        assert_eq!(process_alpha_token("IMPORTANT"), "important");
    }

    #[test]
    fn short_allcaps_spelled_out() {
        assert_eq!(process_alpha_token("UIS"), "U I S");
        assert_eq!(process_alpha_token("TTS"), "T T S");
        assert_eq!(process_alpha_token("DNA"), "D N A");
    }

    #[test]
    fn four_char_allcaps_lowercased() {
        assert_eq!(process_alpha_token("NASA"), "nasa");
        assert_eq!(process_alpha_token("HTTP"), "http");
    }

    #[test]
    fn mixed_case_untouched() {
        assert_eq!(process_alpha_token("Hello"), "Hello");
        assert_eq!(process_alpha_token("iPhone"), "iPhone");
    }

    #[test]
    fn possessive_allcaps_lowercased() {
        assert_eq!(process_alpha_token("AUGMENT's"), "augment's");
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
}
