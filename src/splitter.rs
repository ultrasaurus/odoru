use unicode_segmentation::UnicodeSegmentation;

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

/// Split `text` into sentences using Unicode sentence boundary rules (UAX #29).
///
/// Newlines are always treated as hard sentence breaks, so the caller can
/// force a split at any point by inserting a newline. Within each line,
/// boundaries are detected at `.` `!` `?` — common abbreviations like
/// "Dr.", "Mr.", and "U.S.A." are protected from false splits.
pub fn split(text: &str) -> Vec<String> {
    // Step 1: hide abbreviation periods so the Unicode splitter ignores them.
    let mut protected = text.to_string();
    for abbrev in ABBREVS {
        let pattern = format!("{abbrev}.");
        let replacement = format!("{abbrev}{PERIOD_PLACEHOLDER}");
        protected = protected.replace(&pattern, &replacement);
    }

    // Step 2: split on newlines (hard breaks) then Unicode sentence boundaries.
    let sentences: Vec<String> = protected
        .lines()
        .flat_map(|line| line.unicode_sentences())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.replace(PERIOD_PLACEHOLDER, "."))
        .collect();

    sentences
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── basic correctness ─────────────────────────────────────────────────

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
        assert_eq!(result, vec!["Hello world."]);
    }

    #[test]
    fn split_two_sentences_in_one_line() {
        let result = split("Hello world. How are you?");
        assert_eq!(result, vec!["Hello world.", "How are you?"]);
    }

    #[test]
    fn split_exclamation_and_question_marks() {
        let result = split("Watch out! Are you okay? I think so.");
        assert_eq!(result, vec!["Watch out!", "Are you okay?", "I think so."]);
    }

    // ── newline handling ──────────────────────────────────────────────────

    #[test]
    fn newline_is_always_a_hard_break() {
        let result = split("First line\nSecond line");
        assert_eq!(result, vec!["First line", "Second line"]);
    }

    #[test]
    fn blank_lines_are_skipped() {
        let result = split("First.\n\n\nSecond.");
        assert_eq!(result, vec!["First.", "Second."]);
    }

    #[test]
    fn leading_and_trailing_whitespace_is_stripped() {
        let result = split("  Hello.  \n  World.  ");
        assert_eq!(result, vec!["Hello.", "World."]);
    }

    // ── abbreviations ─────────────────────────────────────────────────────

    #[test]
    fn abbreviation_mr_does_not_cause_false_split() {
        let result = split("Mr. Smith went to the store.");
        assert_eq!(result, vec!["Mr. Smith went to the store."]);
    }

    #[test]
    fn abbreviation_dr_does_not_cause_false_split() {
        let result = split("Dr. Smith made a diagnosis.");
        assert_eq!(result, vec!["Dr. Smith made a diagnosis."]);
    }

    #[test]
    fn abbreviation_at_sentence_end_is_not_split() {
        // Known limitation of the placeholder approach: when a protected
        // abbreviation like "etc." genuinely ends a sentence, the split is
        // missed and the two sentences are merged. For TTS this is acceptable —
        // a slightly long sentence is far better than "Dr." as a lone utterance.
        let result = split("She brought food, drinks, etc. Then she left.");
        assert_eq!(result, vec!["She brought food, drinks, etc. Then she left."]);
    }

    // ── ordering ──────────────────────────────────────────────────────────

    #[test]
    fn split_preserves_sentence_order() {
        let result = split("One. Two. Three.");
        assert_eq!(result, vec!["One.", "Two.", "Three."]);
    }

    #[test]
    fn split_multi_line_multi_sentence_preserves_order() {
        let input = "Hello world. How are you?\nI am fine. Thanks for asking.";
        let result = split(input);
        assert_eq!(result, vec![
            "Hello world.",
            "How are you?",
            "I am fine.",
            "Thanks for asking.",
        ]);
    }

    // ── ellipsis ──────────────────────────────────────────────────────────

    #[test]
    fn ellipsis_does_not_split_mid_thought() {
        let result = split("Wait... are you sure? Yes.");
        assert_eq!(result, vec!["Wait... are you sure?", "Yes."]);
    }
}
