//! Durable job state in an object store.
//!
//! `vibe-service` keeps an in-memory `HashMap` as the hot path; this module
//! is the durability layer behind it. It is written only on genuine state
//! transitions (`Running`/`Done`/`Error`) and read only on a local cache
//! miss (resurrection after instance churn or restart). See
//! `vibe/dev/gcs-job-state.md`.
//!
//! Layout, flat by `job_id`:
//!   {job_id}/status.json     -- StoredStatus (the commit marker for `done`)
//!   {job_id}/audio.wav
//!   {job_id}/transcript.json
//!   {job_id}/report.json
//!
//! One generic `StoreImpl<S>` covers both backends: `GoogleCloudStorage` in
//! production and `InMemory` for tests / local runs without a bucket.

use anyhow::Result;
use async_trait::async_trait;
use bytes::Bytes;
use object_store::{
    gcp::GoogleCloudStorageBuilder, memory::InMemory, path::Path, ObjectStore, ObjectStoreExt,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// The non-payload fields of a job — everything in `JobState` except the wav
/// bytes and alignment data, which live in their own objects.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StoredStatus {
    pub status: String, // "running" | "done" | "error"
    pub name: Option<String>,
    pub seed: Option<u64>,
    pub wall_secs: Option<f64>,
    pub audio_secs: Option<f64>,
    pub rtf: Option<f64>,
    pub error: Option<String>,
}

#[async_trait]
pub trait JobStore: Send + Sync {
    async fn put_status(&self, job_id: &str, status: &StoredStatus) -> Result<()>;
    async fn get_status(&self, job_id: &str) -> Result<Option<StoredStatus>>;
    /// Store/fetch a named payload object under the job (e.g. "audio.wav").
    async fn put_object(&self, job_id: &str, name: &str, bytes: Bytes) -> Result<()>;
    async fn get_object(&self, job_id: &str, name: &str) -> Result<Option<Bytes>>;
}

fn object_path(job_id: &str, name: &str) -> Path {
    Path::from(format!("{job_id}/{name}"))
}

pub struct StoreImpl<S: ObjectStore> {
    store: S,
}

#[async_trait]
impl<S: ObjectStore> JobStore for StoreImpl<S> {
    async fn put_status(&self, job_id: &str, status: &StoredStatus) -> Result<()> {
        let body = serde_json::to_vec(status)?;
        self.put_object(job_id, "status.json", Bytes::from(body)).await
    }

    async fn get_status(&self, job_id: &str) -> Result<Option<StoredStatus>> {
        match self.get_object(job_id, "status.json").await? {
            Some(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            None => Ok(None),
        }
    }

    async fn put_object(&self, job_id: &str, name: &str, bytes: Bytes) -> Result<()> {
        self.store
            .put(&object_path(job_id, name), bytes.into())
            .await?;
        Ok(())
    }

    async fn get_object(&self, job_id: &str, name: &str) -> Result<Option<Bytes>> {
        match self.store.get(&object_path(job_id, name)).await {
            Ok(result) => Ok(Some(result.bytes().await?)),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

/// GCS-backed store. Uses an explicit service-account key file when
/// `key_path` is set (the RunPod / non-GCP path), otherwise relies on
/// ambient credentials (the Cloud Run metadata server).
pub fn gcs_store(bucket: &str, key_path: Option<&str>) -> Result<Arc<dyn JobStore>> {
    let mut builder = GoogleCloudStorageBuilder::new().with_bucket_name(bucket);
    if let Some(path) = key_path {
        builder = builder.with_service_account_path(path);
    }
    Ok(Arc::new(StoreImpl {
        store: builder.build()?,
    }))
}

/// In-memory store for tests and for running without a bucket configured.
pub fn mem_store() -> Arc<dyn JobStore> {
    Arc::new(StoreImpl {
        store: InMemory::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(status: &str) -> StoredStatus {
        StoredStatus {
            status: status.into(),
            name: Some("seg01".into()),
            seed: Some(71463),
            wall_secs: Some(12.5),
            audio_secs: Some(40.0),
            rtf: Some(0.31),
            error: None,
        }
    }

    #[tokio::test]
    async fn status_round_trips() {
        let store = mem_store();
        assert!(store.get_status("job1").await.unwrap().is_none());
        store.put_status("job1", &sample("done")).await.unwrap();
        let got = store.get_status("job1").await.unwrap().unwrap();
        assert_eq!(got.status, "done");
        assert_eq!(got.seed, Some(71463));
    }

    #[tokio::test]
    async fn objects_round_trip_and_miss_is_none() {
        let store = mem_store();
        assert!(store.get_object("job1", "audio.wav").await.unwrap().is_none());
        store
            .put_object("job1", "audio.wav", Bytes::from_static(b"RIFFwav"))
            .await
            .unwrap();
        let got = store.get_object("job1", "audio.wav").await.unwrap().unwrap();
        assert_eq!(&got[..], b"RIFFwav");
    }
}
