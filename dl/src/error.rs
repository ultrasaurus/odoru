use thiserror::Error;

#[derive(Error, Debug)]
pub enum ArticleError {
    #[error("HTTP error: status {0}")]
    HttpError(u16),

    #[error("{}", fetch_error_message(.0))]
    FetchError(#[from] reqwest::Error),

    #[error("Extraction failed: no article content found")]
    ExtractionFailed,

    #[error("Python error: {0}")]
    PythonError(#[from] pyo3::PyErr),
}

fn fetch_error_message(e: &reqwest::Error) -> String {
    let url = e.url().map(|u| u.as_str()).unwrap_or("unknown URL");
    if e.is_timeout() {
        format!("Request timed out: {}", url)
    } else if e.is_connect() {
        format!("Connection failed: {} — check network or URL", url)
    } else if e.is_status() {
        // Shouldn't reach here (we handle HTTP errors separately) but just in case
        format!("HTTP error for {}: {}", url, e)
    } else if e.is_decode() {
        format!("Failed to decode response from {}: {}", url, e)
    } else {
        format!("Fetch error for {}: {}", url, e)
    }
}
