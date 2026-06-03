use scraper::{ElementRef, Html, Selector};

/// Hostnames that use NLS/Augment-style outline HTML structure.
const AUGMENT_HOSTS: &[&str] = &["dougengelbart.org", "www.dougengelbart.org"];

pub fn is_augment_site(url: &str) -> bool {
    let host = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or("");
    let host = host.split(':').next().unwrap_or(host);
    AUGMENT_HOSTS.contains(&host)
}

/// Returns true if this child element should be excluded from extracted text.
/// Strips:
///   - purple* spans (purplename, purplenumber — all nav chrome)
///   - empty <a> tags (bare anchors used as jump targets, not links)
fn is_chrome(el: ElementRef) -> bool {
    let name = el.value().name();
    if name == "span" && el.value().classes().any(|c| c.starts_with("purple")) {
        return true;
    }
    if name == "a" && el.text().collect::<String>().trim().is_empty() {
        return true;
    }
    false
}

/// Extract text from a node, skipping chrome children.
fn text_excluding_chrome(el: ElementRef) -> String {
    el.children()
        .filter_map(|child| {
            if let Some(child_el) = child.value().as_element().and_then(|_| ElementRef::wrap(child)) {
                if is_chrome(child_el) {
                    return None;
                }
                Some(child_el.text().collect::<String>())
            } else {
                child.value().as_text().map(|t| t.to_string())
            }
        })
        .collect::<String>()
}

/// Map an HTML tag name to a markdown heading prefix.
/// Returns None for non-heading tags (e.g. "p").
fn heading_prefix(tag: &str) -> Option<&'static str> {
    match tag {
        "h1" => Some("# "),
        "h2" => Some("## "),
        "h3" => Some("### "),
        "h4" => Some("#### "),
        _ => None,
    }
}

/// Strip sibling chrome elements that sit between block elements.
/// These can't be filtered during child traversal because they're not children
/// of the paragraphs — the HTML5 parser absorbs their text into adjacent blocks.
///
/// Two kinds:
///   - <a ... class="statement-number"></a>  — empty anchor jump targets
///   - <span class="purplenumber ...">...</span> — visible statement numbers
///
/// We parse once with scraper to find them, then remove their outer HTML from
/// the source string before the main parse.
fn strip_sibling_chrome(html: &str) -> String {
    let document = Html::parse_document(html);

    // Collect the outer HTML of all sibling chrome elements
    let mut removals: Vec<String> = Vec::new();

    let anchor_sel = Selector::parse("a.statement-number").unwrap();
    for el in document.select(&anchor_sel) {
        removals.push(el.html());
    }

    let number_sel = Selector::parse("span.purplenumber").unwrap();
    for el in document.select(&number_sel) {
        removals.push(el.html());
    }

    let mut out = html.to_string();
    for removal in removals {
        out = out.replace(&removal, "");
    }
    out
}

/// Extract body content from NLS/Augment-style outline HTML.
/// Returns `(markdown, plain_text)`, or `None` if no outline elements were found.
/// - `markdown`: headings prefixed with `#`/`##` etc., paragraphs as-is.
/// - `plain_text`: all elements as plain text, no markdown syntax.
pub fn extract_content(html: &str) -> Option<(String, String)> {
    let html = strip_sibling_chrome(html);
    let document = Html::parse_document(&html);

    let block_selector = Selector::parse("h1, h2, h3, h4, p").unwrap();

    let mut md_chunks: Vec<String> = Vec::new();
    let mut text_chunks: Vec<String> = Vec::new();

    for el in document.select(&block_selector) {
        let tag = el.value().name();

        // For <p> elements, require a levelN class to filter out page chrome.
        if tag == "p" {
            let has_level = el.value().classes().any(|c| c.starts_with("level"));
            if !has_level {
                continue;
            }
        }

        let text = text_excluding_chrome(el);
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            continue;
        }

        let md_chunk = match heading_prefix(tag) {
            Some(prefix) => format!("{}{}", prefix, text),
            None => text.clone(),
        };

        md_chunks.push(md_chunk);
        text_chunks.push(text);
    }

    if md_chunks.is_empty() {
        None
    } else {
        Some((md_chunks.join("\n\n"), text_chunks.join("\n\n")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_augment_site() {
        assert!(is_augment_site("https://dougengelbart.org/content/view/148/"));
        assert!(is_augment_site("http://www.dougengelbart.org/foo"));
        assert!(!is_augment_site("https://example.com/foo"));
    }

    #[test]
    fn test_strips_sibling_chrome() {
        let html = r##"
            <html><body>
            <div>
                <a name="2" class="statement-number"></a>
                <span class="purplenumber anyprplnum"><a href="#">2</a></span>
                <h1 class="level1">
                    <a name="Abstract" class="statement-name"></a>
                    <span class="purplename hideshow"><a href="#">Abstract</a></span>
                    Abstract
                </h1>
            </div>
            <div>
                <a name="3" class="statement-number"></a>
                <span class="purplenumber anyprplnum"><a href="#">3</a></span>
                <p class="level2">
                    <span class="purplename"><a href="#">AUGMENT</a></span>
                    Body text here.
                </p>
            </div>
            </body></html>
        "##;

        let (markdown, plain_text) = extract_content(html).unwrap();
        let md_chunks: Vec<&str> = markdown.split("\n\n").collect();
        let text_chunks: Vec<&str> = plain_text.split("\n\n").collect();

        assert_eq!(md_chunks.len(), 2);
        assert_eq!(md_chunks[0], "# Abstract");
        assert_eq!(md_chunks[1], "Body text here.");

        assert_eq!(text_chunks.len(), 2);
        assert_eq!(text_chunks[0], "Abstract");
        assert_eq!(text_chunks[1], "Body text here.");
    }

    #[test]
    fn test_real_links_preserved() {
        let html = r##"
            <html><body>
            <p class="level2">
                See <a href="https://example.com">this reference</a> for details.
            </p>
            </body></html>
        "##;
        let (markdown, plain_text) = extract_content(html).unwrap();
        assert_eq!(markdown, "See this reference for details.");
        assert_eq!(plain_text, "See this reference for details.");
    }

    #[test]
    fn test_extract_content_empty() {
        let html = "<html><body><p>No level classes here.</p></body></html>";
        assert!(extract_content(html).is_none(), "expected None for content with no level classes");
    }
}
