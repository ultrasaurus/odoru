use unicode_segmentation::UnicodeSegmentation;
use roman;

/// Placeholder that temporarily replaces periods in known abbreviations so
/// they don't trigger Unicode sentence boundaries. U+FFFE is a non-character
/// guaranteed never to appear in valid text.
const PERIOD_PLACEHOLDER: &str = "\u{FFFE}";

/// Abbreviations whose trailing period must not be treated as a sentence end.
const ABBREVS: &[&str] = &[
    // Titles
    "Mr", "Mrs", "Ms", "Miss", "Dr", "Prof", "Rev", "Sr", "Jr",
    // Geographic
    "St", "Ave", "Blvd", "Rd", "Mt", "Dept",
    // Latin
    "vs", "etc", "e.g", "i.e", "et al",
    // Months (sometimes abbreviated in prose)
    "Jan", "Feb", "Mar", "Apr", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    // Corporate
    "Corp", "Inc", "Ltd", "Est",
    // Citations (e.g. "Vol. 31, No. 7)") — also matches Intl.Segmenter's
    // behavior client-side, which doesn't treat "No." followed by a digit
    // as a sentence end either; see dev/client-server.md.
    "No",
];

/// A sentence with its position in the document structure.
#[derive(Debug, Clone, PartialEq)]
pub struct Sentence {
    pub text: String,
    /// True if this is the last sentence in its paragraph.
    /// Callers can use this to insert a longer pause after paragraph breaks.
    pub paragraph_end: bool,
}

/// Split `text` into sentences, preserving paragraph boundaries.
///
/// Each non-blank line is its own paragraph — blank lines are just
/// separators (allowed but not required). This matches plain text with no
/// blank lines at all (e.g. Odoru's `plain_text`, one paragraph per line) as
/// well as old-style blank-line-separated text. There's no "single newline
/// as a hard break within one paragraph" case here: `to_plain_text()`
/// always joins blocks with `\n\n` and converts in-block soft/hard breaks to
/// spaces, so a bare single `\n` in real plain_text is always a paragraph
/// boundary, never a hard break inside one. (Markdown-level hard breaks are
/// handled separately, at render time, by `collapseHardBreaksToBr` in
/// `markdown.ts` — this splitter only ever sees post-`to_plain_text` text.)
///
/// Within each paragraph, sentence boundaries are detected at `.` `!` `?`
/// with abbreviation protection. The last sentence of each paragraph is
/// tagged with `paragraph_end: true`.
pub fn split(text: &str) -> Vec<Sentence> {
    let paragraphs: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();

    let mut result = Vec::new();

    for paragraph in paragraphs {
        let sentences = split_paragraph(paragraph);
        let count = sentences.len();
        for (i, text) in sentences.into_iter().enumerate() {
            result.push(Sentence {
                text,
                paragraph_end: i == count - 1,
            });
        }
    }

    result
}

/// Split a single paragraph into sentences.
fn split_paragraph(paragraph: &str) -> Vec<String> {
    // Hide abbreviation periods
    let mut protected = paragraph.to_string();
    for abbrev in ABBREVS {
        let pattern = format!("{abbrev}.");
        let replacement = format!("{abbrev}{PERIOD_PLACEHOLDER}");
        protected = protected.replace(&pattern, &replacement);
    }

    // Split on single newlines (hard breaks) then Unicode sentence boundaries.
    let raw: Vec<String> = protected
        .lines()
        .flat_map(|line| recover_dropped_chars(line, line.unicode_sentences()))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.replace(PERIOD_PLACEHOLDER, "."))
        .collect();

    merge_outline_labels(raw)
}

/// Recovers characters `unicode_sentences()` silently drops between
/// consecutive sentence slices — observed with a closing quote immediately
/// after a spaced ellipsis (e.g. `more . . . ". Next.`): neither returned
/// slice contains the `".` between them, so those two characters just
/// vanish if used as-is.
///
/// Walks the sentence slices in order using pointer offsets into `line`
/// (safe since each slice is guaranteed to be a literal substring of it,
/// produced by the same iterator) and reattaches any gap to the end of the
/// preceding sentence — or, if the very first sentence already starts past
/// the beginning of the line, extends that first sentence backward to
/// absorb it. Either way, no character from the original text is ever
/// lost, regardless of which specific punctuation pattern triggers a future
/// instance of this same upstream quirk.
fn recover_dropped_chars<'a>(
    line: &'a str,
    sentences: impl IntoIterator<Item = &'a str>,
) -> Vec<&'a str> {
    let base = line.as_ptr() as usize;
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut cursor = 0usize;

    for s in sentences {
        let start = s.as_ptr() as usize - base;
        let end = start + s.len();
        if start > cursor {
            match ranges.last_mut() {
                Some(last) => last.1 = start, // absorb the gap into the previous sentence
                None => {
                    ranges.push((cursor, end)); // absorb a leading gap into this sentence
                    cursor = end;
                    continue;
                }
            }
        }
        ranges.push((start, end));
        cursor = end;
    }

    ranges.into_iter().map(|(a, b)| &line[a..b]).collect()
}

/// Merge outline-style labels with the sentence that follows them.
///
/// Unicode sentence segmentation splits `"I. Introduction"` into `["I.", "Introduction"]`
/// because the capital `I` after the space triggers a boundary. The same split happens
/// in the browser's `Intl.Segmenter`. By merging on both sides with the same rule we
/// keep server and client indices in sync.
///
/// A sentence is an outline label if it matches: one or more word-chars, then `.`,
/// with at most 4 alphabetic characters (covers `I`–`VIII`, `A`–`Z`, `1`–`99` etc.)
/// and no lowercase letters (avoids merging real sentences like `"Wait."`).
fn merge_outline_labels(sentences: Vec<String>) -> Vec<String> {
    let is_label = |s: &str| -> bool {
        let s = s.trim().trim_end_matches('.');
        if s.is_empty() || !s.chars().all(|c| c.is_alphanumeric()) { return false; }
        let alpha: Vec<char> = s.chars().filter(|c| c.is_alphabetic()).collect();
        if alpha.len() > 4 { return false; }
        if alpha.iter().all(|c| c.is_uppercase()) {
            return true; // I., II., A., XIV., etc.
        }
        if alpha.iter().all(|c| c.is_lowercase()) {
            // Accept lowercase only if it's a valid Roman numeral ≤ 100 (avoids "mix." = 1009)
            let upper = s.to_uppercase();
            return roman::from(&upper).map_or(false, |n| n <= 100);
        }
        false
    };

    let mut out: Vec<String> = Vec::with_capacity(sentences.len());
    let mut iter = sentences.into_iter().peekable();
    while let Some(s) = iter.next() {
        if is_label(&s) {
            if let Some(next) = iter.peek() {
                let merged = format!("{} {}", s.trim_end(), next.trim_start());
                iter.next();
                out.push(merged);
                continue;
            }
        }
        out.push(s);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(sentences: Vec<Sentence>) -> Vec<String> {
        sentences.into_iter().map(|s| s.text).collect()
    }

    fn paragraph_ends(sentences: &[Sentence]) -> Vec<bool> {
        sentences.iter().map(|s| s.paragraph_end).collect()
    }

    // ── split ─────────────────────────────────────────────────────────────

    #[test]
    fn split_empty_string_returns_empty_vec() {
        assert!(split("").is_empty());
    }

    #[test]
    fn split_whitespace_only_returns_empty_vec() {
        assert!(split("   \n   \n   ").is_empty());
    }

    #[test]
    fn split_single_sentence_returns_one_element() {
        let result = split("Hello world.");
        assert_eq!(texts(result.clone()), vec!["Hello world."]);
        assert_eq!(paragraph_ends(&result), vec![true]);
    }

    #[test]
    fn split_two_sentences_in_one_line() {
        let result = split("Hello world. How are you?");
        assert_eq!(texts(result.clone()), vec!["Hello world.", "How are you?"]);
        assert_eq!(paragraph_ends(&result), vec![false, true]);
    }

    #[test]
    fn split_exclamation_and_question_marks() {
        let result = split("Watch out! Are you okay? I think so.");
        assert_eq!(texts(result), vec!["Watch out!", "Are you okay?", "I think so."]);
    }

    // ── paragraph handling ────────────────────────────────────────────────

    #[test]
    fn blank_line_creates_paragraph_boundary() {
        let input = "First paragraph.\n\nSecond paragraph.";
        let result = split(input);
        assert_eq!(texts(result.clone()), vec!["First paragraph.", "Second paragraph."]);
        assert_eq!(paragraph_ends(&result), vec![true, true]);
    }

    #[test]
    fn multiple_blank_lines_still_one_paragraph_boundary() {
        let input = "First.\n\n\n\nSecond.";
        let result = split(input);
        assert_eq!(texts(result.clone()), vec!["First.", "Second."]);
        assert_eq!(paragraph_ends(&result), vec![true, true]);
    }

    #[test]
    fn paragraph_end_only_on_last_sentence_of_paragraph() {
        let input = "One. Two.\n\nThree. Four.";
        let result = split(input);
        assert_eq!(texts(result.clone()), vec!["One.", "Two.", "Three.", "Four."]);
        assert_eq!(paragraph_ends(&result), vec![false, true, false, true]);
    }

    #[test]
    fn single_newline_is_a_paragraph_boundary() {
        // A bare single `\n` never survives from real markdown-derived
        // plain_text (to_plain_text always uses `\n\n` between blocks), so
        // this is always a paragraph boundary — matching Odoru's
        // one-paragraph-per-line `plain_text` format (no blank lines at all).
        let result = split("First line\nSecond line");
        assert_eq!(texts(result.clone()), vec!["First line", "Second line"]);
        assert_eq!(paragraph_ends(&result), vec![true, true]);
    }

    #[test]
    fn no_blank_lines_still_splits_into_paragraphs() {
        // Odoru's plain_text: one paragraph per line, zero blank lines.
        let input = "First paragraph.\nSecond paragraph.\nThird paragraph.";
        let result = split(input);
        assert_eq!(
            texts(result.clone()),
            vec!["First paragraph.", "Second paragraph.", "Third paragraph."]
        );
        assert_eq!(paragraph_ends(&result), vec![true, true, true]);
    }

    // ── abbreviations ─────────────────────────────────────────────────────

    #[test]
    fn abbreviation_mr_does_not_cause_false_split() {
        let result = split("Mr. Smith went to the store.");
        assert_eq!(texts(result), vec!["Mr. Smith went to the store."]);
    }

    #[test]
    fn abbreviation_dr_does_not_cause_false_split() {
        let result = split("Dr. Smith made a diagnosis.");
        assert_eq!(texts(result), vec!["Dr. Smith made a diagnosis."]);
    }

    #[test]
    fn abbreviation_no_before_digit_does_not_cause_false_split() {
        // "Vol." isn't in ABBREVS, so it still splits there (matching
        // Intl.Segmenter, which also breaks after "Vol." here) — only the
        // "No." + digit case is protected, since that's the one place
        // Intl.Segmenter doesn't break but unicode_sentences() used to.
        let result = split("Communications of the ACM (Vol. 31, No. 7).");
        assert_eq!(texts(result), vec![
            "Communications of the ACM (Vol.",
            "31, No. 7).",
        ]);
    }

    #[test]
    fn abbreviation_at_sentence_end_is_not_split() {
        let result = split("She brought food, drinks, etc. Then she left.");
        assert_eq!(texts(result), vec!["She brought food, drinks, etc. Then she left."]);
    }

    // ── ordering ──────────────────────────────────────────────────────────

    #[test]
    fn split_preserves_sentence_order() {
        let result = split("One. Two. Three.");
        assert_eq!(texts(result), vec!["One.", "Two.", "Three."]);
    }

    #[test]
    fn split_multi_paragraph_preserves_order() {
        let input = "Hello world. How are you?\n\nI am fine. Thanks for asking.";
        let result = split(input);
        assert_eq!(texts(result.clone()), vec![
            "Hello world.",
            "How are you?",
            "I am fine.",
            "Thanks for asking.",
        ]);
        assert_eq!(paragraph_ends(&result), vec![false, true, false, true]);
    }

    // ── outline headers ───────────────────────────────────────────────────

    #[test]
    fn outline_header_merged_with_next() {
        let result = split("I. Introduction\n\nII. Methods");
        assert_eq!(texts(result.clone()), vec!["I. Introduction", "II. Methods"]);
        assert_eq!(paragraph_ends(&result), vec![true, true]);
    }

    #[test]
    fn outline_header_lowercase_next_word() {
        // Lowercase next word — unicode_sentences already keeps as one sentence.
        let result = split("I. introduction\n\nII. methods");
        assert_eq!(texts(result.clone()), vec!["I. introduction", "II. methods"]);
    }

    #[test]
    fn outline_alpha_label_merged() {
        let result = split("A. Background\n\nB. Related Work");
        assert_eq!(texts(result.clone()), vec!["A. Background", "B. Related Work"]);
    }

    #[test]
    fn lowercase_roman_label_merged() {
        let result = split("i. Introduction\n\nii. Methods\n\niii. Results");
        assert_eq!(texts(result.clone()), vec!["i. Introduction", "ii. Methods", "iii. Results"]);
    }

    #[test]
    fn non_outline_short_sentence_not_merged() {
        // "Wait." is short but ends with real punctuation after a word — not an outline label.
        let result = split("Wait. Are you sure?");
        assert_eq!(texts(result), vec!["Wait.", "Are you sure?"]);
    }

    // ── footnote / reference markers ─────────────────────────────────────

    #[test]
    fn footnote_marker_splits_into_no_alpha_sentence() {
        // "*1*" splits off as its own sentence in both spaced and unspaced forms.
        let result = split("seem promising.*1*");
        let ts = texts(result);
        assert!(ts.iter().any(|s| s == "seem promising."));
        let no_alpha: Vec<_> = ts.iter().filter(|s| !s.chars().any(|c| c.is_alphabetic())).collect();
        assert_eq!(no_alpha.len(), 1, "expected one no-alpha sentence for the marker");
    }

    #[test]
    fn bracket_reference_splits_into_no_alpha_sentence() {
        let result = split("Next sentence. [12]");
        let ts = texts(result);
        assert!(ts.iter().any(|s| s == "Next sentence."));
        let no_alpha: Vec<_> = ts.iter().filter(|s| !s.chars().any(|c| c.is_alphabetic())).collect();
        assert_eq!(no_alpha.len(), 1);
    }

    // ── ellipsis ──────────────────────────────────────────────────────────

    #[test]
    fn ellipsis_does_not_split_mid_thought() {
        let result = split("Wait... are you sure? Yes.");
        assert_eq!(texts(result), vec!["Wait... are you sure?", "Yes."]);
    }

    #[test]
    fn no_characters_lost_around_quote_after_spaced_ellipsis() {
        // Regression: unicode_sentences() silently drops the closing quote
        // and period here — neither returned slice contains them — unless
        // recover_dropped_chars reattaches the gap. The quote+period belong
        // with the quoted sentence, not the following one.
        let input = "and then we would say, \"But wait, there's more . . . \". \
                      And then we would play peek-a-boo.";
        let result = texts(split(input));
        assert_eq!(
            result,
            vec![
                "and then we would say, \"But wait, there's more . . . \".",
                "And then we would play peek-a-boo.",
            ]
        );
        // No characters lost: concatenating the sentences (with a single
        // space between, matching how they were originally separated)
        // reconstructs the input exactly.
        assert_eq!(result.join(" "), input);
    }
}

