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
/// Paragraphs are separated by one or more blank lines. Within each paragraph,
/// sentence boundaries are detected at `.` `!` `?` with abbreviation protection.
/// Single newlines within a paragraph are treated as hard sentence breaks.
///
/// The last sentence of each paragraph is tagged with `paragraph_end: true`.
pub fn split(text: &str) -> Vec<Sentence> {
    // Split into paragraphs on blank lines (one or more empty lines).
    let paragraphs: Vec<&str> = text
        .split("\n\n")
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
        .flat_map(|line| line.unicode_sentences())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.replace(PERIOD_PLACEHOLDER, "."))
        .collect();

    merge_outline_labels(raw)
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

/// Convenience: return just the sentence strings, dropping structure.
/// Used by callers that don't need paragraph information.
#[allow(dead_code)]
pub fn split_text(text: &str) -> Vec<String> {
    split(text).into_iter().map(|s| s.text).collect()
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

    // ── split_text ────────────────────────────────────────────────────────

    #[test]
    fn split_text_returns_just_strings() {
        let result = split_text("Hello world. How are you?");
        assert_eq!(result, vec!["Hello world.", "How are you?"]);
    }

    #[test]
    fn split_text_matches_split_texts() {
        let input = "One. Two.\n\nThree.";
        let from_split: Vec<String> = split(input).into_iter().map(|s| s.text).collect();
        let from_split_text = split_text(input);
        assert_eq!(from_split, from_split_text);
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
    fn single_newline_is_hard_break_within_paragraph() {
        let result = split("First line\nSecond line");
        assert_eq!(texts(result.clone()), vec!["First line", "Second line"]);
        // Both in same paragraph, only last is paragraph_end
        assert_eq!(paragraph_ends(&result), vec![false, true]);
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

    // ── ellipsis ──────────────────────────────────────────────────────────

    #[test]
    fn ellipsis_does_not_split_mid_thought() {
        let result = split("Wait... are you sure? Yes.");
        assert_eq!(texts(result), vec!["Wait... are you sure?", "Yes."]);
    }
}
