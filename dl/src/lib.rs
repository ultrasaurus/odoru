mod error;
mod fetch;

use pyo3::ffi::c_str;
use pyo3::prelude::*;

pub use error::ArticleError;

pub struct ParsedArticle {
    pub url: String,
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub date: Option<String>,
    pub description: Option<String>,
    pub content: String,
}

pub enum OutputFormat {
    Markdown,
    Text,
}

impl OutputFormat {
    fn as_str(&self) -> &'static str {
        match self {
            OutputFormat::Markdown => "markdown",
            OutputFormat::Text => "txt",
        }
    }
}

fn parse_authors(raw: Option<&str>) -> Vec<String> {
    match raw {
        None | Some("") => vec![],
        Some(s) => {
            let mut seen = std::collections::HashSet::new();
            s.split(';')
                .map(|a| {
                    let name = a.split('·').next().unwrap_or(a);
                    name.trim().to_string()
                })
                .filter(|a| !a.is_empty() && seen.insert(a.clone()))
                .collect()
        }
    }
}

fn setup_python(py: Python) -> PyResult<()> {
    if let Ok(venv) = std::env::var("VIRTUAL_ENV") {
        let version = py.eval(c"__import__('sys').version_info[:2]", None, None)?;
        let (major, minor): (u8, u8) = version.extract()?;
        let site_packages = format!("{}/lib/python{}.{}/site-packages", venv, major, minor);
        let sys = py.import("sys")?;
        sys.getattr("path")?.call_method1("insert", (0, site_packages))?;
    }
    Ok(())
}

pub fn extract(html: &str, url: &str, format: OutputFormat) -> Result<ParsedArticle, ArticleError> {
    Python::attach(|py| {
        setup_python(py)?;

        let module = PyModule::from_code(
            py,
            c_str!(include_str!("parser.py")),
            c"parser.py",
            c"parser",
        )?;

        let result = module
            .getattr("extract")?
            .call1((html, url, format.as_str()))?;

        let success: bool = result.get_item("success")?.extract()?;
        if !success {
            return Err(ArticleError::ExtractionFailed);
        }

        Ok(ParsedArticle {
            url: result.get_item("url")?.extract::<Option<String>>()?.unwrap_or_else(|| url.to_string()),
            title: result.get_item("title")?.extract()?,
            authors: parse_authors(result.get_item("authors")?.extract::<Option<&str>>()?),
            date: result.get_item("date")?.extract()?,
            description: result.get_item("description")?.extract()?,
            content: result.get_item("markdown")?.extract()?,
        })
    })
}

pub fn fetch_and_extract(url: &str, format: OutputFormat) -> Result<ParsedArticle, ArticleError> {
    let html = fetch::fetch(url)?;
    extract(&html, url, format)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_authors_empty() {
        assert_eq!(parse_authors(None), Vec::<String>::new());
        assert_eq!(parse_authors(Some("")), Vec::<String>::new());
    }

    #[test]
    fn test_parse_authors_single() {
        assert_eq!(parse_authors(Some("Alice")), vec!["Alice"]);
    }

    #[test]
    fn test_parse_authors_multiple() {
        assert_eq!(
            parse_authors(Some("Alice; Bob; Carol")),
            vec!["Alice", "Bob", "Carol"]
        );
    }

    #[test]
    fn test_parse_authors_dedup() {
        assert_eq!(
            parse_authors(Some("Alice; Bob; Alice")),
            vec!["Alice", "Bob"]
        );
    }

    #[test]
    fn test_parse_authors_strips_date_fragment() {
        assert_eq!(
            parse_authors(Some("David Temkin · May")),
            vec!["David Temkin"]
        );
    }

    #[test]
    fn test_parse_authors_trims_whitespace() {
        assert_eq!(
            parse_authors(Some("  Alice  ;  Bob  ")),
            vec!["Alice", "Bob"]
        );
    }
}