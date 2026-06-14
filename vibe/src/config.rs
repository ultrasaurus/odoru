use anyhow::{Context, Result};

/// Load `.env` (in the current directory) and read RUNPOD_API_KEY.
pub fn runpod_api_key() -> Result<String> {
    let _ = dotenvy::from_filename(concat!(env!("CARGO_MANIFEST_DIR"), "/.env"));
    std::env::var("RUNPOD_API_KEY").context("RUNPOD_API_KEY not set (check vibe/.env)")
}
