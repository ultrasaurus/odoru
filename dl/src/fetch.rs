use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, USER_AGENT};
use std::time::Duration;
use crate::error::ArticleError;

pub fn fetch(url: &str) -> Result<String, ArticleError> {
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .default_headers(default_headers())
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()?;

    let response = client.get(url).send()?;

    if !response.status().is_success() {
        return Err(ArticleError::HttpError(response.status().as_u16()));
    }

    Ok(response.text()?)
}

fn default_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
             AppleWebKit/537.36 (KHTML, like Gecko) \
             Chrome/120.0.0.0 Safari/537.36",
        ),
    );
    headers.insert(
        ACCEPT,
        HeaderValue::from_static(
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        ),
    );
    headers.insert(
        ACCEPT_LANGUAGE,
        HeaderValue::from_static("en-US,en;q=0.5"),
    );
    headers
}
