//! De-risk binary for GCS-backed job state.
//!
//! Exercises exactly the operations the future `JobStore` needs against a
//! real bucket, using `object_store` with **no explicit credentials** so we
//! verify ambient credential resolution:
//!   - on Cloud Run: the instance metadata server (the path we're proving);
//!   - elsewhere: whatever ADC the environment provides.
//!
//! Bucket comes from `GCS_BUCKET`. Run with `RUST_BACKTRACE=1` if it fails.
//!
//! Sequence: PUT status.json + audio.wav, GET both back (verify bytes),
//! list the `{job_id}/` prefix, delete both, confirm the prefix is empty.

use anyhow::{bail, Context, Result};
use bytes::Bytes;
use object_store::{gcp::GoogleCloudStorageBuilder, path::Path, ObjectStore, ObjectStoreExt};

#[tokio::main]
async fn main() -> Result<()> {
    let bucket = std::env::var("GCS_BUCKET")
        .context("set GCS_BUCKET to the target bucket name")?;
    println!("bucket: {bucket}");

    // Two auth modes under test:
    //   - SA_KEY_PATH set  -> explicit service-account key file (the RunPod /
    //     non-GCP path; this host has no metadata server, so it proves the
    //     key actually authenticates).
    //   - SA_KEY_PATH unset -> ambient resolution (Cloud Run metadata server).
    let mut builder = GoogleCloudStorageBuilder::new().with_bucket_name(&bucket);
    match std::env::var("SA_KEY_PATH") {
        Ok(path) => {
            println!("auth: service-account key file ({path})");
            builder = builder.with_service_account_path(path);
        }
        Err(_) => println!("auth: ambient (metadata server / ADC)"),
    }
    let store = builder
        .build()
        .context("building GCS client (credential resolution happens here)")?;

    // Unique job id so concurrent/repeat runs don't collide.
    let job_id = format!("gcs-eval-{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis());
    let status_path = Path::from(format!("{job_id}/status.json"));
    let wav_path = Path::from(format!("{job_id}/audio.wav"));

    let status_body = Bytes::from_static(br#"{"status":"done","name":"gcs-eval"}"#);
    let wav_body = Bytes::from(vec![0u8; 4096]); // stand-in for wav bytes

    // PUT
    store.put(&status_path, status_body.clone().into()).await
        .context("PUT status.json")?;
    store.put(&wav_path, wav_body.clone().into()).await
        .context("PUT audio.wav")?;
    println!("put: {status_path}, {wav_path}");

    // GET + verify round-trip
    let got_status = store.get(&status_path).await?.bytes().await?;
    let got_wav = store.get(&wav_path).await?.bytes().await?;
    if got_status != status_body {
        bail!("status.json round-trip mismatch");
    }
    if got_wav != wav_body {
        bail!("audio.wav round-trip mismatch ({} bytes)", got_wav.len());
    }
    println!("get: round-trip ok ({} + {} bytes)", got_status.len(), got_wav.len());

    // LIST the job prefix
    let prefix = Path::from(job_id.clone());
    let listed = list_prefix(&store, &prefix).await?;
    println!("list {prefix}/: {} object(s)", listed.len());
    for p in &listed {
        println!("  - {p}");
    }
    if listed.len() != 2 {
        bail!("expected 2 objects under {prefix}/, found {}", listed.len());
    }

    // DELETE
    store.delete(&status_path).await.context("DELETE status.json")?;
    store.delete(&wav_path).await.context("DELETE audio.wav")?;
    let after = list_prefix(&store, &prefix).await?;
    if !after.is_empty() {
        bail!("prefix not empty after delete: {} left", after.len());
    }
    println!("delete: prefix empty");

    println!("\nOK — credential path works for PUT/GET/list/delete.");
    Ok(())
}

async fn list_prefix(store: &dyn ObjectStore, prefix: &Path) -> Result<Vec<Path>> {
    use futures::StreamExt;
    let mut out = Vec::new();
    let mut stream = store.list(Some(prefix));
    while let Some(meta) = stream.next().await {
        out.push(meta?.location);
    }
    Ok(out)
}
