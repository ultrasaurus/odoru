//! Markdown -> plain text conversion, for TTS sentence splitting.
//!
//! Mirrors the inline-stripping behavior of the frontend's `markdown.ts`:
//! bold/italic/inline-code markers are removed and links are reduced to
//! their link text. Fenced code blocks, images, and horizontal rules are
//! dropped entirely (matching `markdown.ts`, which never weaves code-block
//! content into the spoken sentence list). Block-level elements (paragraphs,
//! headings, list items, blockquote text) are joined with blank lines so
//! the result can be split into paragraphs the same way as plain-text input.

use pulldown_cmark::{Event, Parser, Tag, TagEnd};

/// Convert markdown to plain text suitable for sentence splitting and TTS.
pub fn to_plain_text(markdown: &str) -> String {
    let markdown = strip_silent(markdown);

    let mut blocks: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_code_block = false;
    let mut in_image = false;

    for event in Parser::new(&markdown) {
        match event {
            Event::Start(Tag::CodeBlock(_)) => in_code_block = true,
            Event::End(TagEnd::CodeBlock) => in_code_block = false,
            Event::Start(Tag::Image { .. }) => in_image = true,
            Event::End(TagEnd::Image) => in_image = false,
            Event::Text(text) | Event::Code(text) => {
                if !in_code_block && !in_image {
                    current.push_str(&text);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                current.push(' ');
            }
            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::Item
                | TagEnd::TableCell,
            ) => {
                push_block(&mut blocks, &current);
                current.clear();
            }
            _ => {}
        }
    }

    push_block(&mut blocks, &current);

    blocks.join("\n\n")
}

/// Remove "silent" spans — bracketed text immediately followed by a
/// `<!--silent-->` comment, e.g. `[Doug Engelbart]<!--silent-->`. These are
/// displayed in the rendered document but excluded from speech synthesis.
/// See `dev/silent-text.md`.
///
/// Operates line by line: silent spans are stripped from each line, and a
/// line that became empty (or only heading `#` markers) *because* of the
/// stripping is dropped entirely — so a fully-silent heading produces no
/// spoken block. Originally-blank lines are preserved so paragraph
/// boundaries survive.
fn strip_silent(markdown: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in markdown.lines() {
        let removed = remove_silent_spans(line);
        let trimmed = removed.trim();
        let emptied_by_strip = removed != line
            && (trimmed.is_empty() || trimmed.chars().all(|c| c == '#'));
        if emptied_by_strip {
            continue;
        }
        out.push(removed);
    }
    out.join("\n")
}

/// Remove every `[...]<!--silent-->` span from a single line, keeping any
/// other text. A `<!--silent-->` comment not preceded by a bracket span has
/// only the comment removed.
fn remove_silent_spans(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while let Some((start, end)) = find_silent_comment(rest) {
        let before = &rest[..start];
        if before.ends_with(']') {
            if let Some(open) = before.rfind('[') {
                out.push_str(&before[..open]);
                rest = &rest[end..];
                continue;
            }
        }
        out.push_str(before);
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// Find the next `<!--silent-->` comment (whitespace around `silent`
/// tolerated), returning its byte range `(start, end)` within `s`.
fn find_silent_comment(s: &str) -> Option<(usize, usize)> {
    let mut from = 0;
    while let Some(rel) = s[from..].find("<!--") {
        let start = from + rel;
        let inner_start = start + 4;
        match s[inner_start..].find("-->") {
            Some(erel) => {
                let inner_end = inner_start + erel;
                let end = inner_end + 3;
                if s[inner_start..inner_end].trim() == "silent" {
                    return Some((start, end));
                }
                from = end;
            }
            None => return None,
        }
    }
    None
}

/// Trim and collapse internal whitespace runs (left by dropped inline
/// elements like images) before pushing a finished block.
fn push_block(blocks: &mut Vec<String>, text: &str) {
    let collapsed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if !collapsed.is_empty() {
        blocks.push(collapsed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_bold_italic_inline_code() {
        let md = "This is **bold**, *italic*, and `code`.";
        assert_eq!(to_plain_text(md), "This is bold, italic, and code.");
    }

    #[test]
    fn reduces_links_to_text() {
        let md = "See [this reference](https://example.com) for details.";
        assert_eq!(to_plain_text(md), "See this reference for details.");
    }

    #[test]
    fn joins_paragraphs_with_blank_line() {
        let md = "First paragraph.\n\nSecond paragraph.";
        assert_eq!(to_plain_text(md), "First paragraph.\n\nSecond paragraph.");
    }

    #[test]
    fn includes_headings_and_list_items() {
        let md = "# Title\n\n- one\n- two";
        assert_eq!(to_plain_text(md), "Title\n\none\n\ntwo");
    }

    #[test]
    fn skips_fenced_code_blocks() {
        let md = "Before.\n\n```\nlet x = 1;\n```\n\nAfter.";
        assert_eq!(to_plain_text(md), "Before.\n\nAfter.");
    }

    #[test]
    fn skips_images() {
        let md = "Look: ![alt text](pic.png) done.";
        assert_eq!(to_plain_text(md), "Look: done.");
    }

    #[test]
    fn drops_silent_heading() {
        let md = "## [Doug Engelbart]<!--silent-->\n\nHe invented the mouse.";
        assert_eq!(to_plain_text(md), "He invented the mouse.");
    }

    #[test]
    fn drops_standalone_silent_paragraph() {
        let md = "First.\n\n[An aside]<!--silent-->\n\nSecond.";
        assert_eq!(to_plain_text(md), "First.\n\nSecond.");
    }

    #[test]
    fn preserves_paragraph_boundaries_around_silent() {
        // The blank lines separating paragraphs must survive stripping.
        let md = "# [Intro]<!--silent-->\n\nFirst para.\n\nSecond para.";
        assert_eq!(to_plain_text(md), "First para.\n\nSecond para.");
    }

    #[test]
    fn tolerates_whitespace_in_marker() {
        let md = "## [Ted Nelson]<!-- silent -->\n\nHe coined hypertext.";
        assert_eq!(to_plain_text(md), "He coined hypertext.");
    }
}
