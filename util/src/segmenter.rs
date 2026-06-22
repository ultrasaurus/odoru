/// segmenter.rs — split plain-text documents into TTS-ready segments.
///
/// Processing order per document:
///   1. Parse blank-line-separated paragraphs.
///   2. Merge non-sentence-ending paragraphs (headings, fragments) into the
///      following paragraph — prevents short orphan lines becoming segments.
///   3. Greedily accumulate paragraphs into segments, flushing when the next
///      paragraph would push word count above MAX.
///
/// Output: one `String` per segment, paragraphs joined by `\n`. No
/// engine-specific prefix (`Speaker 1:` etc.) — callers add that.

const MIN: usize = 50;
const MAX: usize = 200;

/// Split `text` into TTS segments. Each returned string contains one or more
/// paragraphs joined with `\n`.
pub fn segment(text: &str) -> Vec<String> {
    segment_with_paragraphs(text)
        .into_iter()
        .map(|paragraphs| paragraphs.join("\n"))
        .collect()
}

/// Same segmentation as `segment`, but keeps each segment's constituent
/// paragraph-unit strings separate instead of joining them with `\n`.
///
/// Useful for callers that need to recover per-paragraph boundaries within a
/// segment — e.g. running `splitter::split` on each paragraph-unit
/// individually, since `splitter::split`'s paragraph detection needs a blank
/// line, which doesn't survive the single-`\n` join `segment()` does.
pub fn segment_with_paragraphs(text: &str) -> Vec<Vec<String>> {
    let paragraphs = parse_paragraphs(text);
    let merged = merge_fragments(paragraphs);
    accumulate(merged)
}

// ---------------------------------------------------------------------------
// Step 1: parse blank-line-separated paragraphs
// ---------------------------------------------------------------------------

fn parse_paragraphs(text: &str) -> Vec<String> {
    let mut paragraphs = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            if !current.is_empty() {
                paragraphs.push(current.join(" "));
                current.clear();
            }
        } else {
            current.push(line.trim());
        }
    }
    if !current.is_empty() {
        paragraphs.push(current.join(" "));
    }
    paragraphs
}

// ---------------------------------------------------------------------------
// Step 2: merge headings/fragments into the following paragraph
// ---------------------------------------------------------------------------

/// True if `p` reads as a complete sentence/paragraph — i.e. ends with
/// `.`/`?`/`!`, ignoring any trailing closing-delimiters (`)`, `"`, `'`,
/// `]`) after it. A paragraph like `"...organization.)"` is a complete
/// sentence even though its literal last char is `)`; checking only the
/// last char (as a naive `ends_with` would) misclassifies any
/// parenthetical-/quote-ending paragraph as an unfinished fragment, which
/// then gets wrongly force-merged into the next paragraph by
/// `merge_fragments` — producing a run-on paragraph that splices an aside
/// directly onto an unrelated following sentence/heading with no
/// separation. Confirmed as the cause of an observed TTS truncation —
/// a `(Note: ...)` aside glued directly onto the following heading
/// sentence with no paragraph break.
fn ends_sentence(p: &str) -> bool {
    p.trim_end()
        .trim_end_matches([')', '"', '\'', ']'])
        .ends_with(['.', '?', '!'])
}

fn merge_fragments(paragraphs: Vec<String>) -> Vec<String> {
    let mut merged: Vec<String> = Vec::new();
    let mut carry = String::new();
    for p in paragraphs {
        if carry.is_empty() {
            carry = p;
        } else {
            carry.push(' ');
            carry.push_str(&p);
        }
        if ends_sentence(&carry) {
            merged.push(carry);
            carry = String::new();
        }
    }
    if !carry.is_empty() {
        merged.push(carry);
    }
    merged
}

fn word_count(s: &str) -> usize {
    s.split_whitespace().count()
}

// ---------------------------------------------------------------------------
// Step 3: greedy accumulation into segments
// ---------------------------------------------------------------------------

fn accumulate(paragraphs: Vec<String>) -> Vec<Vec<String>> {
    let mut segments: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut current_wc: usize = 0;

    for p in paragraphs {
        let pw = word_count(&p);
        if current_wc > 0 && current_wc + pw > MAX {
            segments.push(std::mem::take(&mut current));
            current_wc = 0;
        }
        current.push(p);
        current_wc += pw;
        // Flush if we've hit the minimum — avoids holding a complete segment
        // in memory waiting for a paragraph that might never come.
        if current_wc >= MIN && current_wc >= MAX {
            segments.push(std::mem::take(&mut current));
            current_wc = 0;
        }
    }
    if !current.is_empty() {
        segments.push(current);
    }
    segments
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn wc(s: &str) -> usize { s.split_whitespace().count() }

    #[test]
    fn parse_basic() {
        let text = "Hello world.\n\nSecond paragraph.";
        let p = parse_paragraphs(text);
        assert_eq!(p, vec!["Hello world.", "Second paragraph."]);
    }

    #[test]
    fn parse_multiline_paragraph() {
        let text = "Line one\nline two\nline three.\n\nNext.";
        let p = parse_paragraphs(text);
        assert_eq!(p[0], "Line one line two line three.");
    }

    #[test]
    fn merge_heading_into_next() {
        // Heading does not end with sentence punctuation — should merge.
        let paras = vec!["Introduction".to_string(), "This is the body.".to_string()];
        let merged = merge_fragments(paras);
        assert_eq!(merged.len(), 1);
        assert!(merged[0].starts_with("Introduction This is the body."));
    }

    #[test]
    fn merge_two_sentence_paras_stay_separate() {
        let paras = vec!["First sentence.".to_string(), "Second sentence.".to_string()];
        let merged = merge_fragments(paras);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn paren_ending_paragraph_stays_separate_from_next() {
        // Regression: a paragraph ending in ".)" is a complete sentence —
        // ends_sentence must look past the trailing ")" to see the "." and
        // not treat it as an unfinished fragment to merge with the next
        // paragraph (a heading, in the real-world case this came from).
        let paras = vec![
            "(Note: this is an aside.)".to_string(),
            "Statement Numbers and Names.".to_string(),
        ];
        let merged = merge_fragments(paras);
        assert_eq!(merged.len(), 2, "{:?}", merged);
    }

    #[test]
    fn quote_ending_paragraph_stays_separate_from_next() {
        let paras = vec![
            "She said \"hello.\"".to_string(),
            "Then she left.".to_string(),
        ];
        let merged = merge_fragments(paras);
        assert_eq!(merged.len(), 2, "{:?}", merged);
    }

    fn segs_wc(segs: &[Vec<String>]) -> Vec<usize> {
        segs.iter().map(|s| wc(&s.join("\n"))).collect()
    }

    #[test]
    fn accumulate_respects_max() {
        // Each paragraph is 90 words — two fit (180 < 250), three exceed (270 > 250).
        let word90 = "word ".repeat(90).trim().to_string() + ".";
        let paras = vec![word90.clone(), word90.clone(), word90.clone()];
        let segs = accumulate(paras);
        assert_eq!(segs.len(), 2, "should split at MAX boundary: {:?}", segs_wc(&segs));
        assert!(segs_wc(&segs)[0] <= MAX, "first segment {} words", segs_wc(&segs)[0]);
    }

    #[test]
    fn accumulate_no_flush_below_min() {
        // 30-word para: below MIN, should not be flushed alone when next para fits.
        let word30 = "word ".repeat(30).trim().to_string() + ".";
        let word80 = "word ".repeat(80).trim().to_string() + ".";
        let paras = vec![word30.clone(), word80.clone()];
        // 30 + 80 = 110, fits in MAX=250 — should be one segment.
        let segs = accumulate(paras);
        assert_eq!(segs.len(), 1);
    }

    #[test]
    fn seg25_bug_fixed() {
        // Regression: a 137-word para followed by a 239-word para should NOT
        // produce a single 376-word segment. The second paragraph alone
        // (239 words) exceeds MAX=200, but that's expected — a single
        // paragraph too large to combine with anything still gets its own
        // segment rather than being artificially split; the bug being
        // guarded against is unnecessary *merging*, not a lone oversized
        // paragraph.
        let w137: String = "word ".repeat(137).trim().to_string() + ".";
        let w239: String = "word ".repeat(239).trim().to_string() + ".";
        let segs = accumulate(vec![w137, w239]);
        assert_eq!(segs.len(), 2, "should not merge into one segment: {:?}", segs_wc(&segs));
    }

    #[test]
    fn segment_with_paragraphs_matches_segment_when_joined() {
        let doc = "Introduction\n\nThis is the first paragraph with enough words to be meaningful content here yes.\n\nThis is the second paragraph with enough words to be meaningful content here yes.";
        let grouped = segment_with_paragraphs(doc);
        let joined: Vec<String> = grouped.iter().map(|ps| ps.join("\n")).collect();
        assert_eq!(joined, segment(doc));
    }

    #[test]
    fn segment_with_paragraphs_preserves_multiple_paragraphs_per_segment() {
        // Two short paragraphs that fit comfortably under MIN, both ending in
        // sentence punctuation so they don't get fragment-merged — should land
        // in the same segment, with paragraph structure preserved separately.
        let doc = "First short paragraph here with words.\n\nSecond short paragraph here with words.";
        let grouped = segment_with_paragraphs(doc);
        assert_eq!(grouped.len(), 1, "{:?}", grouped);
        assert_eq!(grouped[0].len(), 2, "expected two distinct paragraph units: {:?}", grouped[0]);
    }

    #[test]
    fn segment_integration() {
        // Full pipeline on a small document.
        let doc = "Introduction\n\nThis is the first paragraph with enough words to be meaningful content here yes.\n\nThis is the second paragraph with enough words to be meaningful content here yes.";
        let segs = segment(doc);
        // "Introduction" should be merged into first para.
        assert!(segs[0].starts_with("Introduction"));
    }
}
