//! Slug utilities for export directory names and filenames.

/// Convert a title string into a URL/filename-safe slug.
///
/// - Lowercased
/// - Non-alphanumeric characters (except spaces) stripped
/// - Whitespace runs collapsed and joined with `-`
/// - Truncated to 60 characters
///
/// # Examples
/// ```
/// use util::slug::title_slug;
/// assert_eq!(title_slug("Hello World"), "hello-world");
/// assert_eq!(title_slug("It's a test, really!"), "its-a-test-really");
/// ```
pub fn title_slug(title: &str) -> String {
    let slug = title
        .to_lowercase()
        .replace(|c: char| !c.is_alphanumeric() && c != ' ', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-");
    if slug.len() > 60 {
        slug[..60].to_string()
    } else {
        slug
    }
}

/// Build the export slug used for directory names: `{date}-{title_slug}`.
///
/// Falls back to `"undated"` if `date` is `None`, and `"untitled"` if `title` is `None`.
///
/// # Examples
/// ```
/// use util::slug::export_slug;
/// assert_eq!(export_slug(Some("Hello World"), Some("2024-01-15")), "2024-01-15-hello-world");
/// assert_eq!(export_slug(None, Some("2024-01-15")), "2024-01-15-untitled");
/// assert_eq!(export_slug(Some("My Post"), None), "undated-my-post");
/// ```
pub fn export_slug(title: Option<&str>, date: Option<&str>) -> String {
    let date = date.unwrap_or("undated");
    let slug = title_slug(title.unwrap_or("untitled"));
    format!("{}-{}", date, slug)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── title_slug ────────────────────────────────────────────────────────

    #[test]
    fn title_slug_basic() {
        assert_eq!(title_slug("Hello World"), "hello-world");
    }

    #[test]
    fn title_slug_strips_punctuation() {
        assert_eq!(title_slug("It's a test, really!"), "its-a-test-really");
    }

    #[test]
    fn title_slug_truncates_long_title() {
        let long = "a ".repeat(40); // 80 chars
        let result = title_slug(long.trim());
        assert!(result.len() <= 60, "slug too long: {}", result.len());
    }

    #[test]
    fn title_slug_untitled() {
        assert_eq!(title_slug("untitled"), "untitled");
    }

    // ── export_slug ───────────────────────────────────────────────────────

    #[test]
    fn export_slug_basic() {
        assert_eq!(export_slug(Some("Hello World"), Some("2024-01-15")), "2024-01-15-hello-world");
    }

    #[test]
    fn export_slug_strips_punctuation() {
        assert_eq!(export_slug(Some("It's a test, really!"), Some("2024-03-01")), "2024-03-01-its-a-test-really");
    }

    #[test]
    fn export_slug_truncates_long_title() {
        let long = "word ".repeat(20);
        let result = export_slug(Some(long.trim()), Some("2024-01-01"));
        // date prefix is 11 chars ("2024-01-01-"), slug portion ≤ 60
        assert!(result.len() <= 71, "export_slug too long: {}", result.len());
    }

    #[test]
    fn export_slug_missing_title_uses_untitled() {
        assert_eq!(export_slug(None, Some("2024-06-01")), "2024-06-01-untitled");
    }

    #[test]
    fn export_slug_missing_date_uses_undated() {
        assert_eq!(export_slug(Some("My Post"), None), "undated-my-post");
    }
}
