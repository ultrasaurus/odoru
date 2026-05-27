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
