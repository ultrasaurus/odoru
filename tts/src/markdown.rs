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
    let mut blocks: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_code_block = false;
    let mut in_image = false;

    for event in Parser::new(markdown) {
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
}
