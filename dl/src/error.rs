use thiserror::Error;

#[derive(Error, Debug)]
pub enum ArticleError {
    #[error("HTTP error: status {0}")]
    HttpError(u16),

    #[error("Fetch error: {0}")]
    FetchError(#[from] reqwest::Error),

    #[error("Extraction failed: no article content found")]
    ExtractionFailed,

    #[error("Python error: {0}")]
    PythonError(#[from] pyo3::PyErr),
}
