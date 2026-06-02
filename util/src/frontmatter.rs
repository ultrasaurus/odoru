//! YAML frontmatter parsing for markdown files.
//!
//! Handles the `---` delimited frontmatter used by both article files
//! and voice definitions. Returns the typed header and the body text
//! separately so callers can use each independently.
//!
//! # Example
//!
//! ```
//! use serde::Deserialize;
//! use util::frontmatter::parse;
//!
//! #[derive(Deserialize)]
//! struct Meta { title: String }
//!
//! let src = "---\ntitle: Hello\n---\nBody text.";
//! let (meta, body) = parse::<Meta>(src).unwrap();
//! assert_eq!(meta.title, "Hello");
//! assert_eq!(body.trim(), "Body text.");
//! ```

use anyhow::{bail, Context, Result};
use serde::de::DeserializeOwned;

/// Parse a markdown string with YAML frontmatter.
///
/// The file must begin with `---`, followed by YAML, followed by another
/// `---`. Returns the deserialized header and the body text after the
/// closing delimiter (leading newline stripped).
pub fn parse<T: DeserializeOwned>(src: &str) -> Result<(T, &str)> {
    let src = src.trim_start_matches('\n');

    if !src.starts_with("---") {
        bail!("missing opening '---' frontmatter delimiter");
    }

    // Find the closing `---` (skip the opening one)
    let after_open = &src[3..];
    let close = after_open
        .find("\n---")
        .context("missing closing '---' frontmatter delimiter")?;

    let yaml = &after_open[..close];
    let body = &after_open[close + 4..]; // skip \n---
    let body = body.strip_prefix('\n').unwrap_or(body);

    let header: T = serde_yaml::from_str(yaml)
        .context("failed to parse YAML frontmatter")?;

    Ok((header, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Meta {
        title: String,
        count: u32,
    }

    #[test]
    fn parse_basic_frontmatter() {
        let src = "---\ntitle: Hello\ncount: 3\n---\nBody text here.";
        let (meta, body) = parse::<Meta>(src).unwrap();
        assert_eq!(meta.title, "Hello");
        assert_eq!(meta.count, 3);
        assert_eq!(body, "Body text here.");
    }

    #[test]
    fn parse_body_with_leading_newline_stripped() {
        let src = "---\ntitle: Hi\ncount: 1\n---\n\nParagraph.";
        let (_, body) = parse::<Meta>(src).unwrap();
        assert_eq!(body, "\nParagraph.");
    }

    #[test]
    fn parse_empty_body() {
        let src = "---\ntitle: Hi\ncount: 0\n---\n";
        let (meta, body) = parse::<Meta>(src).unwrap();
        assert_eq!(meta.title, "Hi");
        assert_eq!(body, "");
    }

    #[test]
    fn parse_missing_open_delimiter_errors() {
        let src = "title: Hi\ncount: 1\n---\nBody.";
        assert!(parse::<Meta>(src).is_err());
    }

    #[test]
    fn parse_missing_close_delimiter_errors() {
        let src = "---\ntitle: Hi\ncount: 1\nBody.";
        assert!(parse::<Meta>(src).is_err());
    }

    #[test]
    fn parse_invalid_yaml_errors() {
        let src = "---\n: bad: yaml:\n---\nBody.";
        assert!(parse::<Meta>(src).is_err());
    }
}
