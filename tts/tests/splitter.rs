/// Split `text` into sentences.
///
/// This is a stub that splits on newlines so the bridge layer is testable
/// end-to-end before the real unicode-segmentation implementation is wired in.
/// Replace the body with the full implementation in the next step.
pub fn split(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_empty_string_returns_empty_vec() {
        assert!(split("").is_empty());
    }

    #[test]
    fn split_single_line_returns_one_sentence() {
        let result = split("Hello world.");
        assert_eq!(result, vec!["Hello world."]);
    }

    #[test]
    fn split_multiple_lines_returns_one_per_line() {
        let input = "Hello world.\nHow are you?\nGoodbye.";
        let result = split(input);
        assert_eq!(result, vec!["Hello world.", "How are you?", "Goodbye."]);
    }

    #[test]
    fn split_strips_leading_and_trailing_whitespace() {
        let input = "  Hello.  \n  World.  ";
        let result = split(input);
        assert_eq!(result, vec!["Hello.", "World."]);
    }

    #[test]
    fn split_skips_blank_lines() {
        let input = "First.\n\n\nSecond.";
        let result = split(input);
        assert_eq!(result, vec!["First.", "Second."]);
    }

    #[test]
    fn split_whitespace_only_input_returns_empty_vec() {
        assert!(split("   \n   \n   ").is_empty());
    }

    #[test]
    fn split_preserves_order() {
        let input = "One.\nTwo.\nThree.";
        let result = split(input);
        assert_eq!(result[0], "One.");
        assert_eq!(result[1], "Two.");
        assert_eq!(result[2], "Three.");
    }
}
