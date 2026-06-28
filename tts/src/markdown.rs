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

/// One spoken sentence, with both its plain text (for synthesis indices)
/// and its raw markdown source (for inline-formatted export rendering).
#[derive(Debug, Clone, PartialEq)]
pub struct ExportSentence {
    pub text: String,
    pub markdown_text: String,
    pub paragraph_end: bool,
}

/// Split `markdown` into sentences for static-export rendering, paired with
/// per-block sentence counts so a caller walking the same block structure
/// (e.g. the frontend's `marked.lexer`) knows how many sentences each block
/// contributes — mirrors `markdown.ts`'s `weaveSpans`, but run once at
/// export time instead of per-render in the browser, so the export never
/// needs the wasm splitter.
///
/// Per block, the plain-text split (via `splitter::split`) determines the
/// sentence count; the raw-markdown split is used for the rendered text only
/// when its count matches — otherwise (same fallback `weaveSpans` uses) the
/// plain text is repeated as `markdown_text` for that block, since a count
/// mismatch means the raw split can't be trusted to align sentence-for-
/// sentence with the plain one.
pub fn split_for_export(markdown: &str) -> (Vec<ExportSentence>, Vec<usize>) {
    let markdown = strip_silent(markdown);

    let mut sentences: Vec<ExportSentence> = Vec::new();
    let mut block_lengths: Vec<usize> = Vec::new();

    let mut current_plain = String::new();
    let mut block_start: Option<usize> = None;
    let mut last_end = 0usize;
    let mut in_code_block = false;
    let mut in_image = false;

    for (event, range) in Parser::new(&markdown).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(_)) => in_code_block = true,
            Event::End(TagEnd::CodeBlock) => in_code_block = false,
            Event::Start(Tag::Image { .. }) => in_image = true,
            Event::End(TagEnd::Image) => in_image = false,
            Event::Start(Tag::Paragraph | Tag::Heading { .. } | Tag::Item) => {
                block_start = Some(range.start);
            }
            Event::Text(text) | Event::Code(text) => {
                if !in_code_block && !in_image {
                    current_plain.push_str(&text);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                current_plain.push(' ');
            }
            Event::End(TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item) => {
                if let Some(start) = block_start.take() {
                    let raw = markdown[start..last_end].trim();
                    push_export_block(&mut sentences, &mut block_lengths, &current_plain, raw);
                }
                current_plain.clear();
            }
            _ => {}
        }
        last_end = range.end;
    }

    (sentences, block_lengths)
}

/// Collapse a markdown hard break (line ending in 2+ spaces, or a trailing
/// backslash) to a literal `<br>` so it survives sentence splitting and can
/// still be rendered as a line break; a soft break collapses to a space.
/// Mirrors `collapseHardBreaksToBr` in `markdown.ts`.
fn collapse_hard_breaks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(idx) = rest.find('\n') {
        let before = &rest[..idx];
        if before.ends_with("  ") {
            out.push_str(before.trim_end_matches(' '));
            out.push_str(" <br>");
        } else if before.ends_with('\\') {
            out.push_str(&before[..before.len() - 1]);
            out.push_str("<br>");
        } else {
            out.push_str(before);
            out.push(' ');
        }
        rest = &rest[idx + 1..];
    }
    out.push_str(rest);
    out
}

fn push_export_block(
    sentences: &mut Vec<ExportSentence>,
    block_lengths: &mut Vec<usize>,
    plain: &str,
    raw: &str,
) {
    let plain = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    if plain.is_empty() {
        return;
    }
    let plain_sentences = crate::splitter::split(&plain);
    let raw_collapsed = collapse_hard_breaks(raw);
    let raw_sentences = crate::splitter::split(&raw_collapsed);

    let use_raw = raw_sentences.len() == plain_sentences.len();
    block_lengths.push(plain_sentences.len());
    for (i, s) in plain_sentences.into_iter().enumerate() {
        let markdown_text = if use_raw {
            raw_sentences[i].text.clone()
        } else {
            s.text.clone()
        };
        sentences.push(ExportSentence {
            text: s.text,
            markdown_text,
            paragraph_end: s.paragraph_end,
        });
    }
}

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

    // ── split_for_export ──────────────────────────────────────────────────

    #[test]
    fn export_single_sentence_block_keeps_formatting() {
        let md = "This is **bold** text.";
        let (sentences, block_lengths) = split_for_export(md);
        assert_eq!(block_lengths, vec![1]);
        assert_eq!(sentences[0].text, "This is bold text.");
        assert_eq!(sentences[0].markdown_text, "This is **bold** text.");
    }

    #[test]
    fn export_multiple_sentences_in_one_block() {
        let md = "First sentence. Second **sentence**.";
        let (sentences, block_lengths) = split_for_export(md);
        assert_eq!(block_lengths, vec![2]);
        assert_eq!(sentences[0].text, "First sentence.");
        assert_eq!(sentences[0].markdown_text, "First sentence.");
        assert_eq!(sentences[1].text, "Second sentence.");
        assert_eq!(sentences[1].markdown_text, "Second **sentence**.");
    }

    #[test]
    fn export_heading_and_list_items_are_separate_blocks() {
        let md = "# Title\n\n- one\n- two";
        let (sentences, block_lengths) = split_for_export(md);
        assert_eq!(block_lengths, vec![1, 1, 1]);
        assert_eq!(sentences.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(), vec!["Title", "one", "two"]);
    }

    #[test]
    fn export_paragraph_with_link_preserves_markdown() {
        let md = "See [this reference](https://example.com) for details.";
        let (sentences, block_lengths) = split_for_export(md);
        assert_eq!(block_lengths, vec![1]);
        assert_eq!(sentences[0].text, "See this reference for details.");
        assert_eq!(sentences[0].markdown_text, "See [this reference](https://example.com) for details.");
    }

    #[test]
    fn export_drops_silent_heading_block() {
        let md = "## [Doug Engelbart]<!--silent-->\n\nHe invented the mouse.";
        let (sentences, block_lengths) = split_for_export(md);
        assert_eq!(block_lengths, vec![1]);
        assert_eq!(sentences[0].text, "He invented the mouse.");
    }
}
