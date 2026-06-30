//! Voice upload helpers for the vibe CLI.

use anyhow::{Context, Result};

/// POST a reference wav to the vibe-service `/voices/<name>/<gender>` endpoint.
///
/// `base_url` should already be the resolved, health-checked service URL.
pub async fn upload_wav(
    http: &reqwest::Client,
    base_url: &str,
    name: &str,
    gender: &str,
    bytes: Vec<u8>,
    secret: &Option<String>,
) -> Result<()> {
    let mut req = http
        .post(format!("{base_url}/voices/{name}/{gender}"))
        .body(bytes);
    if let Some(s) = secret {
        req = req.bearer_auth(s);
    }
    let resp = req.send().await.context("POST /voices")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("upload failed: HTTP {status} {body}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::{method, path}};

    #[tokio::test]
    async fn upload_wav_posts_to_correct_path() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/voices/Andy/man"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let http = reqwest::Client::new();
        upload_wav(&http, &server.uri(), "Andy", "man", b"fakewav".to_vec(), &None)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn upload_wav_returns_error_on_failure() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/voices/Andy/man"))
            .respond_with(ResponseTemplate::new(500).set_body_string("oops"))
            .mount(&server)
            .await;

        let http = reqwest::Client::new();
        let err = upload_wav(&http, &server.uri(), "Andy", "man", b"fakewav".to_vec(), &None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("HTTP 500"));
    }
}
