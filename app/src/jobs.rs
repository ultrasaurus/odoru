//! Background synthesis jobs.
//!
//! A job synthesises a full article with a given voice, populating the F5
//! disk cache sentence-by-sentence. The client can reconnect at any time
//! and benefit from cache hits without the job needing to coordinate with
//! live WS sessions — the per-sentence lock in TtsEngine handles that.
//!
//! ## Persistence
//!
//! Each job is written to `~/.odoru/jobs/<id>.json`. On startup the store
//! loads all jobs from disk. Jobs that were `in_progress` at crash are
//! reset to `pending` so they will be re-run (disk cache makes them fast).
//!
//! ## Deduplication
//!
//! `POST /jobs` with the same (text, voice) as an existing pending,
//! in_progress, or done job returns the existing job rather than creating a
//! new one. Error and cancelled jobs can be re-submitted.
//!
//! ## Cancellation
//!
//! Each running job has an `Arc<AtomicBool>` cancel flag stored in memory.
//! `JobStore::cancel()` sets the flag; the synthesis task checks it between
//! sentences and marks the job `Cancelled` when it sees it. The flag is not
//! persisted — cancelled jobs load as `Cancelled` on restart (no re-run).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use tts::TtsEngine;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Pending,
    InProgress,
    Done,
    Error,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    /// Full prefixed voice ID, e.g. "f5:sarah".
    pub voice: String,
    /// Short preview of the text for display in the queue.
    pub text_preview: String,
    pub created_at: DateTime<Utc>,
    pub status: JobStatus,
    pub total_sentences: usize,
    pub completed_sentences: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// JobStore
// ---------------------------------------------------------------------------

pub type SharedJob = Arc<RwLock<Job>>;

pub struct JobStore {
    jobs: Arc<DashMap<String, SharedJob>>,
    /// In-memory cancel flags — not persisted.
    cancel_flags: Arc<DashMap<String, Arc<AtomicBool>>>,
    jobs_dir: PathBuf,
}

impl JobStore {
    /// Load all jobs from `~/.odoru/jobs/`, resetting any in_progress to pending.
    pub fn load() -> anyhow::Result<Self> {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        let jobs_dir = PathBuf::from(home).join(".odoru").join("jobs");
        std::fs::create_dir_all(&jobs_dir)?;

        let jobs: Arc<DashMap<String, SharedJob>> = Arc::new(DashMap::new());

        for entry in std::fs::read_dir(&jobs_dir)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") { continue; }
            match std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str::<Job>(&s).ok()) {
                Some(mut job) => {
                    if job.status == JobStatus::InProgress {
                        job.status = JobStatus::Pending;
                        job.completed_sentences = 0;
                    }
                    eprintln!("[jobs] loaded job {} ({:?})", &job.id[..8], job.status);
                    jobs.insert(job.id.clone(), Arc::new(RwLock::new(job)));
                }
                None => eprintln!("[jobs] skipping unreadable {}", path.display()),
            }
        }

        Ok(Self { jobs, cancel_flags: Arc::new(DashMap::new()), jobs_dir })
    }

    /// Find an existing non-terminal job with the same (text_hash, voice).
    /// Error and Cancelled jobs are excluded so they can be re-submitted.
    pub async fn find_active(&self, text_hash: &str, voice: &str) -> Option<SharedJob> {
        for entry in self.jobs.iter() {
            let job = entry.value().read().await;
            if job.voice == voice
                && entry.key().starts_with(text_hash)
                && !matches!(job.status, JobStatus::Error | JobStatus::Cancelled)
            {
                return Some(entry.value().clone());
            }
        }
        None
    }

    /// Create and persist a new job. Returns the shared handle and its cancel flag.
    pub async fn create(
        &self,
        text: &str,
        voice: &str,
        total_sentences: usize,
    ) -> anyhow::Result<(SharedJob, Arc<AtomicBool>)> {
        let text_hash = text_hash(text);
        let id = format!("{text_hash}-{}", uuid::Uuid::new_v4());
        let text_preview = make_preview(text);
        let job = Job {
            id: id.clone(),
            voice: voice.to_string(),
            text_preview,
            created_at: Utc::now(),
            status: JobStatus::Pending,
            total_sentences,
            completed_sentences: 0,
            error: None,
        };
        let shared = Arc::new(RwLock::new(job));
        let cancel_flag = Arc::new(AtomicBool::new(false));
        self.jobs.insert(id.clone(), shared.clone());
        self.cancel_flags.insert(id, cancel_flag.clone());
        self.persist(&*shared.read().await)?;
        Ok((shared, cancel_flag))
    }

    /// Cancel a job by ID. Sets its cancel flag and marks it Cancelled immediately.
    /// Returns false if the job is not found or already in a terminal state.
    pub async fn cancel(&self, id: &str) -> bool {
        let Some(shared) = self.jobs.get(id).map(|e| e.value().clone()) else {
            return false;
        };
        {
            let job = shared.read().await;
            if matches!(job.status, JobStatus::Done | JobStatus::Error | JobStatus::Cancelled) {
                return false;
            }
        }
        // Signal the running task.
        if let Some(flag) = self.cancel_flags.get(id) {
            flag.store(true, Ordering::Relaxed);
        }
        // Mark immediately so the client sees the change without waiting for
        // the task to notice the flag.
        let mut job = shared.write().await;
        job.status = JobStatus::Cancelled;
        let _ = self.persist(&job);
        true
    }

    pub fn all(&self) -> Vec<SharedJob> {
        self.jobs.iter().map(|e| e.value().clone()).collect()
    }

    pub fn get(&self, id: &str) -> Option<SharedJob> {
        self.jobs.get(id).map(|e| e.value().clone())
    }

    /// True if a live cancel flag exists for this job (i.e. a task is running).
    pub fn has_cancel_flag(&self, id: &str) -> bool {
        self.cancel_flags.contains_key(id)
    }

    /// Register a fresh cancel flag for a job being (re)started.
    pub fn register_cancel_flag(&self, id: &str) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        self.cancel_flags.insert(id.to_string(), flag.clone());
        flag
    }

    pub fn persist(&self, job: &Job) -> anyhow::Result<()> {
        let path = self.jobs_dir.join(format!("{}.json", job.id));
        let json = serde_json::to_string_pretty(job)?;
        std::fs::write(path, json)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Job runner
// ---------------------------------------------------------------------------

/// Spawn a tokio task that drives synthesis for the job. Checks the cancel
/// flag between sentences and stops gracefully if it is set.
pub fn spawn_job(
    shared: SharedJob,
    cancel_flag: Arc<AtomicBool>,
    text: String,
    voice_name: String,
    engine: Arc<TtsEngine>,
    store: Arc<JobStore>,
) {
    tokio::spawn(async move {
        {
            let mut job = shared.write().await;
            if job.status == JobStatus::Cancelled { return; }
            job.status = JobStatus::InProgress;
            job.completed_sentences = 0;
            if let Err(e) = store.persist(&job) {
                eprintln!("[jobs] persist error: {e}");
            }
        }

        let job_id = shared.read().await.id.clone();
        eprintln!("[jobs] starting job {}", &job_id[..8]);

        let mut stream = engine.synthesize(&text, &voice_name);
        let mut completed = 0usize;

        while let Some(result) = stream.next().await {
            // Check cancel flag between sentences.
            if cancel_flag.load(Ordering::Relaxed) {
                eprintln!("[jobs] cancelled job {}", &job_id[..8]);
                let mut job = shared.write().await;
                job.status = JobStatus::Cancelled;
                let _ = store.persist(&job);
                return;
            }
            match result {
                Ok(_seg) => {
                    completed += 1;
                    let mut job = shared.write().await;
                    job.completed_sentences = completed;
                    if let Err(e) = store.persist(&job) {
                        eprintln!("[jobs] persist error: {e}");
                    }
                }
                Err(e) => {
                    eprintln!("[jobs] synthesis error in job {}: {e}", &job_id[..8]);
                    let mut job = shared.write().await;
                    job.status = JobStatus::Error;
                    job.error = Some(e.to_string());
                    let _ = store.persist(&job);
                    return;
                }
            }
        }

        let mut job = shared.write().await;
        job.status = JobStatus::Done;
        eprintln!("[jobs] done job {} ({completed} sentences)", &job_id[..8]);
        let _ = store.persist(&job);
    });
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub fn text_hash(text: &str) -> String {
    let mut h = Sha256::new();
    h.update(text.as_bytes());
    format!("{:.16x}", h.finalize())
}

fn make_preview(text: &str) -> String {
    let preview: String = text.chars().take(80).collect();
    if text.len() > 80 {
        format!("{preview}…")
    } else {
        preview
    }
}
