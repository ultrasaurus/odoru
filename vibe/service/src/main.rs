mod jobstore;
mod watchdog;

use anyhow::Result;
use bytes::Bytes;
use jobstore::{JobStore, StoredStatus};
use axum::{
    body::Body,
    extract::{Path, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};
use tokio::task::JoinSet;
use tracing::{info, warn, error};
use uuid::Uuid;
use watchdog::ActivityTracker;

#[derive(Clone)]
struct AppState {
    secret: Option<String>,
    gpu_info: Arc<String>,
    tracker: ActivityTracker,
    jobs: Arc<RwLock<HashMap<String, JobState>>>,
    /// batch_id -> job_ids. In-memory only — accepted gap (dev/parallel.md):
    /// individual job_ids are durable via `store`/resurrect, but a batch_id
    /// doesn't survive instance churn. Client mitigation: it already has the
    /// job_ids array from POST /batches's response, so it can fall back to
    /// per-job polling if GET /batches/:id 404s.
    batches: Arc<RwLock<HashMap<String, Vec<String>>>>,
    store: Arc<dyn JobStore>,
    job_semaphore: Arc<Semaphore>,
    heartbeat_secs: u64,
}

#[derive(Clone, Serialize)]
struct AlignData {
    transcript: forced_alignment::transcript::Transcript,
    report: forced_alignment::transcript::AlignReport,
}

#[derive(Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum JobState {
    Pending {
        name: Option<String>,
    },
    Running {
        name: Option<String>,
    },
    Done {
        name: Option<String>,
        seed: u64,
        wall_secs: f64,
        audio_secs: Option<f64>,
        rtf: Option<f64>,
        #[serde(skip)]
        wav_bytes: Arc<Vec<u8>>,
        #[serde(skip)]
        align: Option<AlignData>,
    },
    Error {
        name: Option<String>,
        error: String,
    },
}


#[derive(Deserialize)]
struct JobRequest {
    text: String,
    #[serde(default = "default_seed")]
    seed: u64,
    #[serde(default = "default_speaker")]
    speaker: String,
    #[serde(default = "default_cfg_scale")]
    cfg_scale: f64,
    /// Sampling temperature. When set, the inference script enables sampling
    /// (do_sample); when absent, generation is greedy/deterministic.
    #[serde(default)]
    temp: Option<f64>,
    /// Voice speed factor applied to the reference audio. <1 slows the cloned
    /// voice, >1 speeds it up. Absent (or 1.0) leaves it unchanged.
    #[serde(default)]
    speed: Option<f64>,
    name: Option<String>,
}

#[derive(Serialize)]
struct JobCreated {
    job_id: String,
    name: Option<String>,
}

#[derive(Deserialize)]
struct BenchRequest {
    #[serde(default = "default_bench_batch_sizes")]
    batch_sizes: String,
    #[serde(default = "default_speaker")]
    speaker: String,
    #[serde(default = "default_cfg_scale")]
    cfg_scale: f64,
    #[serde(default = "default_seed")]
    seed: u64,
}

#[derive(Serialize)]
struct BenchStarted {
    request_id: String,
    gcs_prefix: Option<String>,
    log_url: String,
}

fn default_bench_batch_sizes() -> String { "1,2,4,8".into() }

fn default_seed() -> u64 { 71463 }
fn default_speaker() -> String { "Sarah".into() }
fn default_cfg_scale() -> f64 { 1.3 }

#[derive(Deserialize, Serialize, Clone)]
struct BatchSegment {
    text: String,
    name: String,
}

/// One shared set of generation knobs for the whole batch — matches current
/// workflow (one set of knobs per CLI invocation already, even across a
/// `for seg in ...` shell loop); see dev/parallel.md "Stage 3 implementation
/// plan" for why per-segment overrides weren't added.
#[derive(Deserialize)]
struct BatchRequest {
    segments: Vec<BatchSegment>,
    #[serde(default = "default_seed")]
    seed: u64,
    #[serde(default = "default_speaker")]
    speaker: String,
    #[serde(default = "default_cfg_scale")]
    cfg_scale: f64,
    #[serde(default)]
    temp: Option<f64>,
    #[serde(default)]
    speed: Option<f64>,
}

#[derive(Serialize)]
struct BatchCreated {
    batch_id: String,
    job_ids: Vec<String>,
}

/// Deliberately thin: pending/running/done/error, "any job errored" reported
/// as error plus which ones. No partial-status semantics yet — we haven't
/// had a single-job failure to learn what richer reporting should look like
/// (see dev/parallel.md).
#[derive(Serialize)]
struct BatchStatus {
    status: &'static str,
    job_ids: Vec<String>,
    errored_job_ids: Vec<String>,
}

async fn health_handler(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ready",
        "gpu": *state.gpu_info,
    }))
}

async fn log_handler(
    State(state): State<AppState>,
    Path(request_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    if !request_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') || request_id.len() > 64 {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let log_path = format!("/tmp/{request_id}.log");
    match tokio::fs::read_to_string(&log_path).await {
        Ok(contents) => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, "text/plain")],
            contents,
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Upload a reference voice wav for VibeVoice to use, without baking it
/// into the (public) Docker image. Persists only for the pod's lifetime —
/// re-upload after creating a new pod. Written as
/// `en-<name>_<gender>.wav`, matching VibeVoice's own naming convention
/// (e.g. `en-Sarah_woman.wav`), so `--speaker_names <name>` resolves it.
async fn upload_voice_handler(
    State(state): State<AppState>,
    Path((name, gender)): Path<(String, String)>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if !authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let valid_segment = |s: &str| !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric());
    if !valid_segment(&name) || !valid_segment(&gender) {
        return (StatusCode::BAD_REQUEST, "name and gender must be non-empty alphanumeric").into_response();
    }

    let path = format!("/workspace/VibeVoice/demo/voices/en-{name}_{gender}.wav");
    match tokio::fs::write(&path, &body).await {
        Ok(()) => {
            info!(name = %name, gender = %gender, bytes = body.len(), "uploaded voice to {path}");
            StatusCode::OK.into_response()
        }
        Err(e) => {
            warn!("writing {path}: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn create_job_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<JobRequest>,
) -> Response {
    if !authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let job_id = Uuid::new_v4().to_string();
    let name = req.name.clone();

    let text_words = req.text.split_whitespace().count();
    info!(job_id = %job_id, name = ?name, words = text_words, "job created");

    state.jobs.write().await.insert(job_id.clone(), JobState::Pending { name: name.clone() });

    let state2 = state.clone();
    let job_id2 = job_id.clone();
    tokio::spawn(async move {
        run_job(state2, job_id2, req).await;
    });

    Json(JobCreated { job_id, name }).into_response()
}

/// Triggers `demo/batch_bench.py` (dev/parallel.md "Max batch size is
/// unknown") in the background. Exists because Cloud Run has no shell/exec
/// into the container — this is the only way to run the benchmark there.
/// Progress is tailable via the existing `/log/:request_id`; results (wavs +
/// `batch_bench_runs.jsonl`) are uploaded to GCS by the script itself (ambient
/// auth, same bucket/mechanism as `jobstore.rs`) since there's no other way
/// to get files out of a Cloud Run instance. On RunPod this still works the
/// same way, or can be run directly over SSH instead — this endpoint doesn't
/// change that path.
async fn create_bench_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<BenchRequest>,
) -> Response {
    if !authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let request_id = Uuid::new_v4().to_string();
    let gcs_bucket = std::env::var("GCS_BUCKET").ok();
    let gcs_prefix = gcs_bucket.as_ref().map(|_| format!("bench/{request_id}"));

    info!(request_id = %request_id, batch_sizes = %req.batch_sizes, "bench started");

    let log_path = format!("/tmp/{request_id}.log");
    let output_dir = format!("/tmp/bench_{request_id}");
    let results_jsonl = format!("/tmp/bench_{request_id}.jsonl");
    let gcs_prefix2 = gcs_prefix.clone();
    let request_id2 = request_id.clone();
    let tracker = state.tracker.clone();
    let job_semaphore = state.job_semaphore.clone();

    tokio::spawn(async move {
        // Shares the same semaphore as run_job: a bench sweep already
        // internally exercises batch sizes up to the largest requested, so
        // letting it run concurrently with regular /jobs synth calls would
        // stack untested concurrency on top of untested batch size on the
        // same GPU — exactly what this tool exists to measure cleanly, not
        // contaminate. MAX_CONCURRENT_JOBS=1 makes this fully exclusive.
        let _permit = match job_semaphore.acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => return,
        };

        // Without this, the RunPod idle watchdog (3 min of active_requests==0)
        // could stop the pod mid-sweep, since otherwise nothing here looks any
        // different from idle to it. Same guard pattern as run_job.
        tracker.touch();
        tracker.increment();
        struct DecrementGuard(ActivityTracker);
        impl Drop for DecrementGuard {
            fn drop(&mut self) { self.0.touch(); self.0.decrement(); }
        }
        let _guard = DecrementGuard(tracker);

        if let Err(e) = run_bench_inner(&req, &output_dir, &results_jsonl, &log_path, gcs_prefix2.as_deref()).await {
            error!(request_id = %request_id2, "bench failed: {e:#}");
        }
    });

    Json(BenchStarted {
        log_url: format!("/log/{request_id}"),
        request_id,
        gcs_prefix,
    }).into_response()
}

async fn run_bench_inner(
    req: &BenchRequest,
    output_dir: &str,
    results_jsonl: &str,
    log_path: &str,
    gcs_prefix: Option<&str>,
) -> Result<()> {
    let log_file = std::fs::File::create(log_path)?;
    let log_file2 = log_file.try_clone()?;

    let mut args: Vec<String> = vec![
        "demo/batch_bench.py".into(),
        "--batch_sizes".into(), req.batch_sizes.clone(),
        "--speaker".into(), req.speaker.clone(),
        "--cfg_scale".into(), req.cfg_scale.to_string(),
        "--seed".into(), req.seed.to_string(),
        "--output_dir".into(), output_dir.into(),
        "--results_jsonl".into(), results_jsonl.into(),
    ];
    if let Some(prefix) = gcs_prefix {
        args.push("--gcs_prefix".into());
        args.push(prefix.into());
    }

    let mut child = tokio::process::Command::new("python3")
        .args(&args)
        .stdout(std::process::Stdio::from(log_file))
        .stderr(std::process::Stdio::from(log_file2))
        .current_dir("/workspace/VibeVoice")
        .spawn()?;

    let status = child.wait().await?;
    if !status.success() {
        let tail = tail_log(log_path).await;
        anyhow::bail!("batch_bench.py exited {status}\n{tail}");
    }
    Ok(())
}

/// On a local cache miss, rebuild job status from the durable store
/// (resurrection after instance churn / restart) and insert a marker into the
/// cache so later polls are fast. Payload objects (wav/transcript/report) are
/// fetched lazily by their own handlers, so the resurrected `Done` carries an
/// empty wav and `None` align.
///
/// Stuck-running rule: a job found still at `running` in the store has no
/// committed result and its instance is gone, so it is treated as dead — a
/// terminal error rather than an indefinite `running` that never resolves.
async fn resurrect(state: &AppState, job_id: &str) {
    if state.jobs.read().await.contains_key(job_id) {
        return;
    }
    let status = match state.store.get_status(job_id).await {
        Ok(Some(s)) => s,
        Ok(None) => return, // unknown job → stays absent → 404
        Err(e) => {
            warn!(job_id = %job_id, error = %e, "resurrect: store read failed");
            return;
        }
    };
    let job = match status.status.as_str() {
        "done" => JobState::Done {
            name: status.name,
            seed: status.seed.unwrap_or(0),
            wall_secs: status.wall_secs.unwrap_or(0.0),
            audio_secs: status.audio_secs,
            rtf: status.rtf,
            wav_bytes: Arc::new(vec![]), // fetched lazily from store
            align: None,                 // fetched lazily from store
        },
        "error" => JobState::Error {
            name: status.name,
            error: status.error.unwrap_or_else(|| "job failed".into()),
        },
        _ => {
            info!(job_id = %job_id, "resurrect: job interrupted mid-run, marking dead");
            JobState::Error {
                name: status.name,
                error: "job interrupted before completion (no result persisted); resubmit".into(),
            }
        }
    };
    state.jobs.write().await.entry(job_id.to_string()).or_insert(job);
}

async fn get_job_handler(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    resurrect(&state, &job_id).await;
    let jobs = state.jobs.read().await;
    match jobs.get(&job_id) {
        None => StatusCode::NOT_FOUND.into_response(),
        Some(job) => Json(job.clone()).into_response(),
    }
}

async fn get_job_wav_handler(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    resurrect(&state, &job_id).await;

    // Decide from the cache without holding the lock across the (possible)
    // store fetch.
    enum WavSource {
        Cached(Arc<Vec<u8>>),
        FetchFromStore,
        Conflict(&'static str),
        NotFound,
    }
    let source = {
        let jobs = state.jobs.read().await;
        match jobs.get(&job_id) {
            None => WavSource::NotFound,
            // Locally-run job: wav is in the cache.
            Some(JobState::Done { wav_bytes, .. }) if !wav_bytes.is_empty() => {
                WavSource::Cached(wav_bytes.clone())
            }
            // Resurrected Done: wav not in cache, pull it from the store.
            Some(JobState::Done { .. }) => WavSource::FetchFromStore,
            Some(JobState::Pending { .. }) => WavSource::Conflict("pending"),
            Some(JobState::Running { .. }) => WavSource::Conflict("running"),
            Some(JobState::Error { .. }) => WavSource::Conflict("error"),
        }
    };

    let wav_bytes = match source {
        WavSource::Cached(b) => b.as_ref().clone(),
        WavSource::FetchFromStore => match state.store.get_object(&job_id, "audio.wav").await {
            Ok(Some(b)) => b.to_vec(),
            Ok(None) => return StatusCode::NOT_FOUND.into_response(),
            Err(e) => {
                error!(job_id = %job_id, error = %e, "wav fetch from store failed");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        },
        WavSource::Conflict(s) => {
            return (StatusCode::CONFLICT, Json(serde_json::json!({ "status": s }))).into_response()
        }
        WavSource::NotFound => return StatusCode::NOT_FOUND.into_response(),
    };

    (
        [(axum::http::header::CONTENT_TYPE, HeaderValue::from_static("audio/wav"))],
        Body::from(wav_bytes),
    )
        .into_response()
}

async fn get_job_transcript_handler(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    resurrect(&state, &job_id).await;
    serve_align_object(&state, &job_id, "transcript.json", |a| &a.transcript).await
}

/// Shared logic for the transcript/report handlers: serve from the cached
/// `AlignData` when present, otherwise fetch the stored JSON object (a
/// resurrected job, or one whose align data isn't in memory). A `Done` job
/// with no align object yields 404, matching prior behaviour when alignment
/// failed.
async fn serve_align_object<T: Serialize>(
    state: &AppState,
    job_id: &str,
    object_name: &str,
    pick: impl Fn(&AlignData) -> &T,
) -> Response {
    enum Decision {
        Serve(Vec<u8>),
        FetchFromStore,
        Conflict,
        NotFound,
    }
    let decision = {
        let jobs = state.jobs.read().await;
        match jobs.get(job_id) {
            Some(JobState::Done { align: Some(a), .. }) => {
                match serde_json::to_vec(pick(a)) {
                    Ok(bytes) => Decision::Serve(bytes),
                    Err(_) => Decision::NotFound,
                }
            }
            Some(JobState::Done { align: None, .. }) => Decision::FetchFromStore,
            Some(_) => Decision::Conflict,
            None => Decision::NotFound,
        }
    };
    let bytes = match decision {
        Decision::Serve(b) => b,
        Decision::FetchFromStore => match state.store.get_object(job_id, object_name).await {
            Ok(Some(b)) => b.to_vec(),
            Ok(None) => return StatusCode::NOT_FOUND.into_response(),
            Err(e) => {
                error!(job_id = %job_id, object = object_name, error = %e, "align fetch from store failed");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        },
        Decision::Conflict => return StatusCode::CONFLICT.into_response(),
        Decision::NotFound => return StatusCode::NOT_FOUND.into_response(),
    };
    (
        [(axum::http::header::CONTENT_TYPE, HeaderValue::from_static("application/json"))],
        Body::from(bytes),
    )
        .into_response()
}

async fn get_job_report_handler(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    resurrect(&state, &job_id).await;
    serve_align_object(&state, &job_id, "report.json", |a| &a.report).await
}

/// Strip "Speaker N: " prefixes from each line, then join into a single string
/// suitable for the forced aligner (which expects plain spoken text).
fn strip_speaker_prefixes(text: &str) -> String {
    text.lines()
        .map(|line| {
            // Match "Speaker <digits>: " at the start of any line.
            if let Some(rest) = line.strip_prefix("Speaker ") {
                if let Some(idx) = rest.find(": ") {
                    let tag = &rest[..idx];
                    if tag.chars().all(|c| c.is_ascii_digit()) {
                        return rest[idx + 2..].to_string();
                    }
                }
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

async fn run_alignment(wav_bytes: &[u8], text: &str, name: &str) -> Option<AlignData> {
    info!(name = %name, "alignment starting");
    // Write wav to a temp file so forced_alignment::audio::load_audio can read it.
    let tmp_path = std::path::PathBuf::from(format!("/tmp/align_{}.wav", Uuid::new_v4()));
    if let Err(e) = tokio::fs::write(&tmp_path, wav_bytes).await {
        error!(name = %name, error = %e, "alignment: failed to write temp wav");
        return None;
    }

    let align_text = strip_speaker_prefixes(text);
    let start = std::time::Instant::now();
    let result = tokio::task::spawn_blocking(move || {
        let samples = forced_alignment::audio::load_audio(&tmp_path, forced_alignment::SAMPLE_RATE)?;
        let _ = std::fs::remove_file(&tmp_path);
        forced_alignment::align(&samples, &align_text)
    })
    .await;
    let align_secs = start.elapsed().as_secs_f64();
    match result {
        Ok(Ok((transcript, report))) => {
            info!(
                name = %name,
                align_secs = format!("{align_secs:.1}"),
                suspects = report.suspect.len(),
                filtered = report.filtered.len(),
                "alignment done"
            );
            Some(AlignData { transcript, report })
        }
        Ok(Err(e)) => {
            error!(name = %name, error = %e, "alignment failed");
            None
        }
        Err(e) => {
            error!(name = %name, error = %e, "alignment task panicked");
            None
        }
    }
}

/// Best-effort write of the status marker to the durable store. A failure
/// only costs durability (the job still lives in the in-memory cache), so we
/// log and carry on rather than failing the job.
async fn persist_status(state: &AppState, job_id: &str, status: StoredStatus) {
    if let Err(e) = state.store.put_status(job_id, &status).await {
        warn!(job_id = %job_id, error = %e, "failed to persist job status to store");
    }
}

/// Persist alignment payloads (transcript + report) as their own objects.
async fn persist_align(state: &AppState, job_id: &str, a: &AlignData) -> Result<()> {
    state.store
        .put_object(job_id, "transcript.json", Bytes::from(serde_json::to_vec(&a.transcript)?))
        .await?;
    state.store
        .put_object(job_id, "report.json", Bytes::from(serde_json::to_vec(&a.report)?))
        .await?;
    Ok(())
}

async fn run_job(state: AppState, job_id: String, req: JobRequest) {
    let name = req.name.as_deref().unwrap_or("(unnamed)");

    // Wait for a free synth slot before claiming Running. Held until this
    // function returns, so it's released whether the job succeeds or
    // errors. While waiting, the job stays Pending — not Running, not
    // stuck — see dev/parallel.md.
    let _permit = match state.job_semaphore.clone().acquire_owned().await {
        Ok(permit) => permit,
        Err(_) => return, // semaphore closed; service is shutting down
    };

    {
        let mut jobs = state.jobs.write().await;
        if let Some(j) = jobs.get_mut(&job_id) {
            *j = JobState::Running { name: req.name.clone() };
        }
    }
    persist_status(&state, &job_id, StoredStatus {
        status: "running".into(),
        name: req.name.clone(),
        seed: None,
        wall_secs: None,
        audio_secs: None,
        rtf: None,
        error: None,
    }).await;

    info!(job_id = %job_id, name = %name, seed = req.seed, cfg = req.cfg_scale, "job running");

    state.tracker.touch();
    state.tracker.increment();

    struct DecrementGuard(ActivityTracker);
    impl Drop for DecrementGuard {
        fn drop(&mut self) { self.0.touch(); self.0.decrement(); }
    }
    let _guard = DecrementGuard(state.tracker.clone());

    let request_id = Uuid::new_v4().to_string();
    let txt_path = format!("/tmp/{request_id}.txt");
    let out_dir = format!("/tmp/{request_id}_out");
    let log_path = format!("/tmp/{request_id}.log");

    let start = std::time::Instant::now();

    // Log a heartbeat every `heartbeat_secs` while inference is running
    // (default 60s; override with HEARTBEAT_SECS for concurrency testing on
    // segments too short to ever hit the default — see dev/parallel.md).
    let heartbeat_job_id = job_id.clone();
    let heartbeat_name = name.to_string();
    let heartbeat_secs = state.heartbeat_secs;
    let (heartbeat_cancel_tx, mut heartbeat_cancel_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(heartbeat_secs));
        interval.tick().await; // skip the immediate first tick
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    // elapsed is approximate; this runs until the cancel fires
                    let gpu_mem = query_gpu_memory().await;
                    tracing::info!(job_id = %heartbeat_job_id, name = %heartbeat_name, gpu_mem = %gpu_mem, "job still running");
                }
                _ = &mut heartbeat_cancel_rx => break,
            }
        }
    });

    let result = async {
        tokio::fs::write(&txt_path, &req.text).await?;
        tokio::fs::create_dir_all(&out_dir).await?;
        run_inference_inner(&req, &txt_path, &out_dir, &log_path, &request_id).await
    }
    .await;

    let _ = heartbeat_cancel_tx.send(());

    match result {
        Err(e) => {
            warn!(job_id = %job_id, name = %name, error = %e, "job failed");
            {
                let mut jobs = state.jobs.write().await;
                if let Some(j) = jobs.get_mut(&job_id) {
                    *j = JobState::Error { name: req.name.clone(), error: e.to_string() };
                } else {
                    warn!(job_id = %job_id, name = %name, "job_id not found in map when storing error");
                }
            }
            persist_status(&state, &job_id, StoredStatus {
                status: "error".into(),
                name: req.name.clone(),
                seed: None,
                wall_secs: None,
                audio_secs: None,
                rtf: None,
                error: Some(e.to_string()),
            }).await;
        }
        Ok((wav_bytes, seed_used)) => {
            let wall = start.elapsed().as_secs_f64();
            let audio_secs = wav_duration_secs(&wav_bytes);
            let rtf = audio_secs.map(|d| wall / d);
            info!(
                job_id = %job_id, name = %name, seed = seed_used,
                wall = format!("{wall:.1}s"),
                rtf = rtf.map(|r| format!("{r:.3}")).unwrap_or_default(),
                "job done"
            );

            // Run forced alignment on GPU. Non-fatal: synthesis result stands even if
            // alignment fails.
            let align = run_alignment(&wav_bytes, &req.text, name).await;

            // Persist payload objects to the durable store BEFORE the status
            // marker, so a `done` status never points at a missing wav. See
            // gcs-job-state.md (write-ordering / atomicity).
            if let Err(e) = state.store
                .put_object(&job_id, "audio.wav", Bytes::from(wav_bytes.clone()))
                .await
            {
                warn!(job_id = %job_id, error = %e, "failed to persist wav to store");
            }
            if let Some(a) = &align {
                if let Err(e) = persist_align(&state, &job_id, a).await {
                    warn!(job_id = %job_id, error = %e, "failed to persist alignment to store");
                }
            }

            {
                let mut jobs = state.jobs.write().await;
                if let Some(j) = jobs.get_mut(&job_id) {
                    *j = JobState::Done {
                        name: req.name.clone(),
                        seed: seed_used,
                        wall_secs: wall,
                        audio_secs,
                        rtf,
                        wav_bytes: Arc::new(wav_bytes),
                        align,
                    };
                } else {
                    warn!(job_id = %job_id, name = %name, "job_id not found in map when storing result");
                }
            }

            // Commit marker last.
            persist_status(&state, &job_id, StoredStatus {
                status: "done".into(),
                name: req.name.clone(),
                seed: Some(seed_used),
                wall_secs: Some(wall),
                audio_secs,
                rtf,
                error: None,
            }).await;
        }
    }
}

async fn run_inference_inner(
    req: &JobRequest,
    txt_path: &str,
    out_dir: &str,
    log_path: &str,
    request_id: &str,
) -> Result<(Vec<u8>, u64)> {
    let log_file = std::fs::File::create(log_path)?;
    let log_file2 = log_file.try_clone()?;

    let mut args: Vec<String> = vec![
        "/workspace/VibeVoice/demo/inference_from_file.py".into(),
        "--model_path".into(), "vibevoice/VibeVoice-1.5B".into(),
        "--txt_path".into(), txt_path.into(),
        "--speaker_names".into(), req.speaker.clone(),
        "--cfg_scale".into(), req.cfg_scale.to_string(),
        "--seed".into(), req.seed.to_string(),
        "--output_dir".into(), out_dir.into(),
    ];
    // Optional knobs: only pass when provided so the script keeps its
    // defaults (greedy decoding; speed unchanged) otherwise.
    if let Some(temp) = req.temp {
        args.push("--temp".into());
        args.push(temp.to_string());
    }
    if let Some(speed) = req.speed {
        args.push("--speed".into());
        args.push(speed.to_string());
    }

    let mut child = tokio::process::Command::new("python3")
        .args(&args)
        .stdout(std::process::Stdio::from(log_file))
        .stderr(std::process::Stdio::from(log_file2))
        .current_dir("/workspace/VibeVoice")
        .spawn()?;

    let status = child.wait().await?;
    if !status.success() {
        let tail = tail_log(log_path).await;
        anyhow::bail!("inference process exited {status}\n{tail}");
    }

    let log_contents = tokio::fs::read_to_string(log_path).await.unwrap_or_default();
    let seed_used = log_contents
        .lines()
        .find(|l| l.contains("Seed used:"))
        .and_then(|l| l.split_whitespace().last())
        .and_then(|s| s.parse().ok())
        .unwrap_or(req.seed);

    let mut entries = tokio::fs::read_dir(out_dir).await?;
    let mut wav_path = None;
    while let Some(entry) = entries.next_entry().await? {
        if entry.path().extension().and_then(|e| e.to_str()) == Some("wav") {
            wav_path = Some(entry.path());
            break;
        }
    }
    let wav_path = wav_path.ok_or_else(|| anyhow::anyhow!("no wav found in {out_dir}"))?;
    let wav_bytes = tokio::fs::read(&wav_path).await?;

    info!("request_id={request_id} wav={} bytes", wav_bytes.len());
    Ok((wav_bytes, seed_used))
}

/// `POST /batches` — submits N segments as one batched `generate()` call.
/// Reuses the per-segment `job_id`/`JobState`/`JobStore` machinery so
/// `/jobs/:id`, `/jobs/:id/wav`, `/jobs/:id/transcript`, `/jobs/:id/report`
/// don't need to change shape at all — only the synth step is batched and
/// the result gets demuxed back into N pre-existing job records. See
/// dev/parallel.md "Stage 3 implementation plan".
async fn create_batch_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<BatchRequest>,
) -> Response {
    if !authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if req.segments.is_empty() {
        return (StatusCode::BAD_REQUEST, "segments must be non-empty").into_response();
    }

    let batch_id = Uuid::new_v4().to_string();
    let job_ids: Vec<String> = req.segments.iter().map(|_| Uuid::new_v4().to_string()).collect();

    {
        let mut jobs = state.jobs.write().await;
        for (job_id, seg) in job_ids.iter().zip(&req.segments) {
            jobs.insert(job_id.clone(), JobState::Pending { name: Some(seg.name.clone()) });
        }
    }
    state.batches.write().await.insert(batch_id.clone(), job_ids.clone());

    info!(batch_id = %batch_id, segments = req.segments.len(), "batch created");

    let state2 = state.clone();
    let job_ids2 = job_ids.clone();
    tokio::spawn(async move {
        run_batch_job(state2, job_ids2, req).await;
    });

    Json(BatchCreated { batch_id, job_ids }).into_response()
}

/// `GET /batches/:id` — thin aggregation over the batch's job_ids (see
/// `BatchStatus` for why the semantics are kept minimal). 404 if the
/// in-memory batch map doesn't have this id (e.g. instance churned mid-batch
/// — accepted gap, see `AppState::batches`); the client already has the
/// job_ids from `POST /batches`'s response and should fall back to polling
/// them individually via the existing `/jobs/:id`, which does survive churn.
async fn get_batch_handler(
    State(state): State<AppState>,
    Path(batch_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let job_ids = match state.batches.read().await.get(&batch_id) {
        Some(ids) => ids.clone(),
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    let mut current = Vec::with_capacity(job_ids.len());
    for job_id in &job_ids {
        resurrect(&state, job_id).await;
        current.push(state.jobs.read().await.get(job_id).cloned());
    }

    let (status, errored) = aggregate_batch_status(&job_ids, &current);
    Json(BatchStatus { status, job_ids, errored_job_ids: errored }).into_response()
}

/// Pure aggregation logic for `GET /batches/:id`, factored out of the
/// handler so it's unit-testable without spinning up jobs/resurrect/HTTP.
/// `current[i]` is the looked-up state for `job_ids[i]` (`None` = not found
/// at all, distinct from `Pending`).
fn aggregate_batch_status(job_ids: &[String], current: &[Option<JobState>]) -> (&'static str, Vec<String>) {
    let mut errored = Vec::new();
    let mut done_count = 0;
    let mut running_count = 0;
    for (job_id, state) in job_ids.iter().zip(current) {
        match state {
            Some(JobState::Error { .. }) => errored.push(job_id.clone()),
            Some(JobState::Done { .. }) => done_count += 1,
            Some(JobState::Running { .. }) => running_count += 1,
            Some(JobState::Pending { .. }) | None => {}
        }
    }

    let status = if !errored.is_empty() {
        "error"
    } else if done_count == job_ids.len() {
        "done"
    } else if running_count > 0 || done_count > 0 {
        // Something has started or finished, but not everything — distinct
        // from "pending" (nothing has started yet). Fixes a bug caught
        // while writing tests for this: the original version OR'd Pending
        // into the same bucket as Running, which made the "pending" status
        // unreachable (any all-Pending batch reported "running" instead).
        "running"
    } else {
        "pending"
    };

    (status, errored)
}

async fn run_batch_job(state: AppState, job_ids: Vec<String>, req: BatchRequest) {
    // Shares the same semaphore as run_job/run_bench — deliberately, not by
    // default. Real batching's whole point is one fused call avoiding GPU
    // time-slicing; running multiple concurrent batch calls would reintroduce
    // that exact contention one level up (N batches time-slicing instead of N
    // segments). MAX_CONCURRENT_JOBS=1 here means "one batch call owns the
    // GPU at a time" — batch size, not process count, is the scaling lever.
    // See dev/parallel.md section 5.
    let _permit = match state.job_semaphore.clone().acquire_owned().await {
        Ok(permit) => permit,
        Err(_) => return,
    };

    for (job_id, seg) in job_ids.iter().zip(&req.segments) {
        {
            let mut jobs = state.jobs.write().await;
            if let Some(j) = jobs.get_mut(job_id) {
                *j = JobState::Running { name: Some(seg.name.clone()) };
            }
        }
        persist_status(&state, job_id, StoredStatus {
            status: "running".into(),
            name: Some(seg.name.clone()),
            seed: None,
            wall_secs: None,
            audio_secs: None,
            rtf: None,
            error: None,
        }).await;
    }

    info!(segments = req.segments.len(), seed = req.seed, cfg = req.cfg_scale, "batch running");

    // Increment once per segment (not once per batch call) — matches the
    // per-segment semantics run_job already uses for ActivityTracker.
    state.tracker.touch();
    for _ in &req.segments {
        state.tracker.increment();
    }
    struct DecrementGuard(ActivityTracker, usize);
    impl Drop for DecrementGuard {
        fn drop(&mut self) {
            self.0.touch();
            for _ in 0..self.1 {
                self.0.decrement();
            }
        }
    }
    let _guard = DecrementGuard(state.tracker.clone(), req.segments.len());

    let request_id = Uuid::new_v4().to_string();
    let out_dir = format!("/tmp/{request_id}_out");
    let log_path = format!("/tmp/{request_id}.log");

    let start = std::time::Instant::now();

    // Same heartbeat pattern as run_job — batches can run longer than a
    // single segment, so this matters at least as much here.
    let heartbeat_request_id = request_id.clone();
    let heartbeat_segments = req.segments.len();
    let heartbeat_secs = state.heartbeat_secs;
    let (heartbeat_cancel_tx, mut heartbeat_cancel_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(heartbeat_secs));
        interval.tick().await;
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let gpu_mem = query_gpu_memory().await;
                    tracing::info!(request_id = %heartbeat_request_id, segments = heartbeat_segments, gpu_mem = %gpu_mem, "batch still running");
                }
                _ = &mut heartbeat_cancel_rx => break,
            }
        }
    });

    let result = async {
        tokio::fs::create_dir_all(&out_dir).await?;
        run_batch_inference_inner(&req, &out_dir, &log_path).await
    }
    .await;

    let _ = heartbeat_cancel_tx.send(());

    match result {
        Err(e) => {
            warn!(error = %e, "batch failed");
            for (job_id, seg) in job_ids.iter().zip(&req.segments) {
                let mut jobs = state.jobs.write().await;
                if let Some(j) = jobs.get_mut(job_id) {
                    *j = JobState::Error { name: Some(seg.name.clone()), error: e.to_string() };
                }
                drop(jobs);
                persist_status(&state, job_id, StoredStatus {
                    status: "error".into(),
                    name: Some(seg.name.clone()),
                    seed: None,
                    wall_secs: None,
                    audio_secs: None,
                    rtf: None,
                    error: Some(e.to_string()),
                }).await;
            }
        }
        Ok((wav_by_name, seed_used)) => {
            let wall = start.elapsed().as_secs_f64();

            // Alignment fired concurrently across the batch, not in a
            // sequential loop — see dev/parallel.md section 4. CPU-side,
            // already shown to handle genuine N-way overlap (cloudrun.md).
            let mut align_tasks: JoinSet<(String, Option<AlignData>)> = JoinSet::new();
            for seg in &req.segments {
                if let Some(wav_bytes) = wav_by_name.get(&seg.name).cloned() {
                    let name = seg.name.clone();
                    let text = seg.text.clone();
                    align_tasks.spawn(async move {
                        let align = run_alignment(&wav_bytes, &text, &name).await;
                        (name, align)
                    });
                }
            }
            let mut align_by_name: HashMap<String, Option<AlignData>> = HashMap::new();
            while let Some(res) = align_tasks.join_next().await {
                match res {
                    Ok((name, align)) => { align_by_name.insert(name, align); }
                    Err(e) => warn!(error = %e, "alignment task join failed"),
                }
            }

            for (job_id, seg) in job_ids.iter().zip(&req.segments) {
                let Some(wav_bytes) = wav_by_name.get(&seg.name).cloned() else {
                    warn!(job_id = %job_id, name = %seg.name, "no wav produced for segment");
                    let mut jobs = state.jobs.write().await;
                    if let Some(j) = jobs.get_mut(job_id) {
                        *j = JobState::Error {
                            name: Some(seg.name.clone()),
                            error: "no audio output for this segment".into(),
                        };
                    }
                    drop(jobs);
                    persist_status(&state, job_id, StoredStatus {
                        status: "error".into(),
                        name: Some(seg.name.clone()),
                        seed: None,
                        wall_secs: None,
                        audio_secs: None,
                        rtf: None,
                        error: Some("no audio output for this segment".into()),
                    }).await;
                    continue;
                };
                let audio_secs = wav_duration_secs(&wav_bytes);
                let rtf = audio_secs.map(|d| wall / d);
                let align = align_by_name.remove(&seg.name).flatten();

                if let Err(e) = state.store
                    .put_object(job_id, "audio.wav", Bytes::from(wav_bytes.clone()))
                    .await
                {
                    warn!(job_id = %job_id, error = %e, "failed to persist wav to store");
                }
                if let Some(a) = &align {
                    if let Err(e) = persist_align(&state, job_id, a).await {
                        warn!(job_id = %job_id, error = %e, "failed to persist alignment to store");
                    }
                }

                {
                    let mut jobs = state.jobs.write().await;
                    if let Some(j) = jobs.get_mut(job_id) {
                        *j = JobState::Done {
                            name: Some(seg.name.clone()),
                            seed: seed_used,
                            wall_secs: wall,
                            audio_secs,
                            rtf,
                            wav_bytes: Arc::new(wav_bytes),
                            align,
                        };
                    }
                }
                persist_status(&state, job_id, StoredStatus {
                    status: "done".into(),
                    name: Some(seg.name.clone()),
                    seed: Some(seed_used),
                    wall_secs: Some(wall),
                    audio_secs,
                    rtf,
                    error: None,
                }).await;
            }
            info!(segments = req.segments.len(), wall = format!("{wall:.1}s"), "batch done");
        }
    }
}

/// Spawns `batch_inference.py`, writing the batch request as JSON to its
/// stdin (not a temp file — the `/tmp/<id>.txt` pattern in
/// `run_inference_inner` exists only because `inference_from_file.py`'s CLI
/// expects `--txt_path`, inherited from the upstream demo script; the new
/// script has no such interface to match, and stdin avoids shell-escaping
/// problems for arbitrary segment text). Returns each segment's wav bytes
/// keyed by name, plus the shared seed actually used.
async fn run_batch_inference_inner(
    req: &BatchRequest,
    out_dir: &str,
    log_path: &str,
) -> Result<(HashMap<String, Vec<u8>>, u64)> {
    let log_file = std::fs::File::create(log_path)?;
    let log_file2 = log_file.try_clone()?;

    let stdin_payload = serde_json::json!({
        "segments": req.segments,
        "seed": req.seed,
        "speaker": req.speaker,
        "cfg_scale": req.cfg_scale,
        "temp": req.temp,
        "speed": req.speed,
    });

    let mut child = tokio::process::Command::new("python3")
        .args([
            "/workspace/VibeVoice/demo/batch_inference.py",
            "--model_path", "vibevoice/VibeVoice-1.5B",
            "--output_dir", out_dir,
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::from(log_file))
        .stderr(std::process::Stdio::from(log_file2))
        .current_dir("/workspace/VibeVoice")
        .spawn()?;

    {
        use tokio::io::AsyncWriteExt;
        let mut stdin = child.stdin.take().ok_or_else(|| anyhow::anyhow!("failed to open child stdin"))?;
        stdin.write_all(serde_json::to_vec(&stdin_payload)?.as_slice()).await?;
        stdin.shutdown().await?;
    }

    let status = child.wait().await?;
    if !status.success() {
        let tail = tail_log(log_path).await;
        anyhow::bail!("batch_inference.py exited {status}\n{tail}");
    }

    let log_contents = tokio::fs::read_to_string(log_path).await.unwrap_or_default();
    let seed_used = log_contents
        .lines()
        .find(|l| l.contains("Seed used:"))
        .and_then(|l| l.split_whitespace().last())
        .and_then(|s| s.parse().ok())
        .unwrap_or(req.seed);

    let mut wav_by_name = HashMap::new();
    for seg in &req.segments {
        let wav_path = std::path::Path::new(out_dir).join(format!("{}.wav", seg.name));
        match tokio::fs::read(&wav_path).await {
            Ok(bytes) => { wav_by_name.insert(seg.name.clone(), bytes); }
            Err(e) => warn!(name = %seg.name, error = %e, "no wav found for segment"),
        }
    }

    Ok((wav_by_name, seed_used))
}

async fn tail_log(log_path: &str) -> String {
    tokio::fs::read_to_string(log_path)
        .await
        .unwrap_or_default()
        .lines()
        .rev()
        .take(40)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n")
}

fn wav_duration_secs(bytes: &[u8]) -> Option<f64> {
    let cursor = std::io::Cursor::new(bytes);
    let reader = hound::WavReader::new(cursor).ok()?;
    let spec = reader.spec();
    let dur = reader.len() as f64 / spec.sample_rate as f64 / spec.channels as f64;
    Some(dur)
}

fn authorized(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(secret) = &state.secret else { return true };
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|provided| provided == secret)
        .unwrap_or(false)
}


fn query_gpu_info() -> String {
    std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=name,memory.total", "--format=csv,noheader"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Best-effort current VRAM usage, for per-job heartbeat logging during
/// concurrency testing (see dev/parallel.md). Same silent-fallback style as
/// `query_gpu_info`; uses `tokio::process::Command` since this runs inside
/// an async heartbeat loop rather than once at startup.
async fn query_gpu_memory() -> String {
    tokio::process::Command::new("nvidia-smi")
        .args(["--query-gpu=memory.used,memory.total", "--format=csv,noheader"])
        .output()
        .await
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Resets the idle watchdog's timer on every request — status polls and
/// result downloads count as activity too, not just the inference job
/// itself. Without this, the watchdog could stop the pod while a client is
/// still mid-download right after a job finishes, since only `run_job`
/// touched the tracker before.
async fn touch_activity(State(state): State<AppState>, request: Request, next: Next) -> Response {
    state.tracker.touch();
    next.run(request).await
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let _ = dotenvy::dotenv();

    let secret = std::env::var("VIBE_SERVICE_SECRET").ok();
    if secret.is_none() {
        warn!("VIBE_SERVICE_SECRET not set — service is unauthenticated");
    }

    let gpu_info = query_gpu_info();
    info!("GPU: {gpu_info}");

    let tracker = ActivityTracker::default();
    watchdog::spawn_idle_watchdog(tracker.clone());

    // Cap how many synth subprocesses run at once on this instance. Default
    // 1 preserves prior single-job behavior; deploys targeting GPUs with
    // VRAM headroom (e.g. Blackwell) raise this alongside Cloud Run
    // --concurrency. See dev/parallel.md.
    let max_concurrent_jobs: usize = std::env::var("MAX_CONCURRENT_JOBS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    info!(max_concurrent_jobs, "job concurrency limit");

    let heartbeat_secs: u64 = std::env::var("HEARTBEAT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);

    // Durable job state. With GCS_BUCKET set, state survives instance churn /
    // restart; without it, falls back to an in-memory store (same behaviour
    // as before, not durable).
    let store: Arc<dyn JobStore> = match std::env::var("GCS_BUCKET") {
        Ok(bucket) => {
            // GCS_SA_KEY_PATH: explicit key file for non-GCP hosts (RunPod).
            // Unset on Cloud Run, where ambient metadata creds are used.
            let key_path = std::env::var("GCS_SA_KEY_PATH").ok();
            info!(
                bucket = %bucket,
                key = key_path.as_deref().unwrap_or("ambient"),
                "job state: GCS-backed"
            );
            jobstore::gcs_store(&bucket, key_path.as_deref())?
        }
        Err(_) => {
            warn!("GCS_BUCKET not set — job state is in-memory only (not durable)");
            jobstore::mem_store()
        }
    };

    let state = AppState {
        secret,
        gpu_info: Arc::new(gpu_info),
        tracker,
        jobs: Arc::new(RwLock::new(HashMap::new())),
        batches: Arc::new(RwLock::new(HashMap::new())),
        store,
        job_semaphore: Arc::new(Semaphore::new(max_concurrent_jobs)),
        heartbeat_secs,
    };

    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/jobs", post(create_job_handler))
        .route("/batches", post(create_batch_handler))
        .route("/batches/:batch_id", get(get_batch_handler))
        .route("/bench", post(create_bench_handler))
        .route("/jobs/:job_id", get(get_job_handler))
        .route("/jobs/:job_id/wav", get(get_job_wav_handler))
        .route("/jobs/:job_id/transcript", get(get_job_transcript_handler))
        .route("/jobs/:job_id/report", get(get_job_report_handler))
        .route("/log/:request_id", get(log_handler))
        .route("/voices/:name/:gender", post(upload_voice_handler))
        .layer(middleware::from_fn_with_state(state.clone(), touch_activity))
        .with_state(state);

    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let addr = format!("0.0.0.0:{port}");
    info!("vibe-service listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AppState backed by an in-memory store — no GCS, no HTTP.
    fn test_state() -> AppState {
        AppState {
            secret: None,
            gpu_info: Arc::new("test-gpu".into()),
            tracker: ActivityTracker::default(),
            jobs: Arc::new(RwLock::new(HashMap::new())),
            batches: Arc::new(RwLock::new(HashMap::new())),
            store: jobstore::mem_store(),
            job_semaphore: Arc::new(Semaphore::new(8)),
            heartbeat_secs: 60,
        }
    }

    fn done_status() -> StoredStatus {
        StoredStatus {
            status: "done".into(),
            name: Some("seg01".into()),
            seed: Some(71463),
            wall_secs: Some(12.5),
            audio_secs: Some(40.0),
            rtf: Some(0.31),
            error: None,
        }
    }

    // ---- resurrection ----

    #[tokio::test]
    async fn resurrect_unknown_job_leaves_cache_empty() {
        let state = test_state();
        resurrect(&state, "nope").await;
        assert!(state.jobs.read().await.get("nope").is_none());
    }

    #[tokio::test]
    async fn resurrect_done_rebuilds_done_marker() {
        let state = test_state();
        state.store.put_status("j1", &done_status()).await.unwrap();

        resurrect(&state, "j1").await;

        let jobs = state.jobs.read().await;
        match jobs.get("j1") {
            Some(JobState::Done { name, seed, wall_secs, audio_secs, rtf, wav_bytes, align }) => {
                assert_eq!(name.as_deref(), Some("seg01"));
                assert_eq!(*seed, 71463);
                assert_eq!(*wall_secs, 12.5);
                assert_eq!(*audio_secs, Some(40.0));
                assert_eq!(*rtf, Some(0.31));
                // payloads are fetched lazily by their handlers, not in the marker
                assert!(wav_bytes.is_empty());
                assert!(align.is_none());
            }
            _ => panic!("expected Done marker"),
        }
    }

    #[tokio::test]
    async fn resurrect_error_rebuilds_error() {
        let state = test_state();
        let mut s = done_status();
        s.status = "error".into();
        s.error = Some("boom".into());
        state.store.put_status("j1", &s).await.unwrap();

        resurrect(&state, "j1").await;

        let jobs = state.jobs.read().await;
        match jobs.get("j1") {
            Some(JobState::Error { error, .. }) => assert_eq!(error, "boom"),
            _ => panic!("expected Error"),
        }
    }

    /// Stuck-running rule: a job still at `running` in the store (instance
    /// died before committing a result) resurrects as a terminal Error, never
    /// an indefinite Running.
    #[tokio::test]
    async fn resurrect_running_is_marked_dead() {
        let state = test_state();
        let mut s = done_status();
        s.status = "running".into();
        state.store.put_status("j1", &s).await.unwrap();

        resurrect(&state, "j1").await;

        let jobs = state.jobs.read().await;
        match jobs.get("j1") {
            Some(JobState::Error { error, .. }) => assert!(error.contains("interrupted")),
            _ => panic!("expected terminal Error for stuck running"),
        }
    }

    #[tokio::test]
    async fn resurrect_does_not_clobber_live_cache() {
        let state = test_state();
        // A locally-run job is Running in cache; the store has nothing.
        state.jobs.write().await.insert(
            "j1".into(),
            JobState::Running { name: Some("seg01".into()) },
        );

        resurrect(&state, "j1").await;

        // Must stay Running — resurrection only fills a *miss*.
        let jobs = state.jobs.read().await;
        match jobs.get("j1") {
            Some(JobState::Running { .. }) => {}
            _ => panic!("resurrect clobbered live cache"),
        }
    }

    // ---- normal-path persistence contract ----

    /// Mirrors what run_job's Done branch persists, then asserts a fresh
    /// instance (empty cache) can both resurrect the marker and fetch the wav
    /// payload from the store — the contract the wav handler relies on.
    #[tokio::test]
    async fn done_payload_and_marker_survive_to_fresh_instance() {
        let writer = test_state();
        let wav = b"RIFF....fake-wav".to_vec();
        // payload first, status marker last (commit ordering)
        writer.store.put_object("j1", "audio.wav", Bytes::from(wav.clone())).await.unwrap();
        writer.store.put_status("j1", &done_status()).await.unwrap();

        // Fresh instance shares only the durable store, not the cache.
        let reader = AppState { jobs: Arc::new(RwLock::new(HashMap::new())), ..writer.clone() };
        assert!(reader.jobs.read().await.is_empty());

        resurrect(&reader, "j1").await;
        assert!(matches!(reader.jobs.read().await.get("j1"), Some(JobState::Done { .. })));

        let fetched = reader.store.get_object("j1", "audio.wav").await.unwrap();
        assert_eq!(fetched.as_deref(), Some(&wav[..]));
        // no alignment was persisted → transcript object is absent
        assert!(reader.store.get_object("j1", "transcript.json").await.unwrap().is_none());
    }

    // ---- batch status aggregation ----

    fn done_job() -> JobState {
        JobState::Done {
            name: None, seed: 71463, wall_secs: 1.0, audio_secs: Some(1.0), rtf: Some(1.0),
            wav_bytes: Arc::new(vec![]), align: None,
        }
    }

    fn running_job() -> JobState {
        JobState::Running { name: None }
    }

    fn pending_job() -> JobState {
        JobState::Pending { name: None }
    }

    fn error_job() -> JobState {
        JobState::Error { name: None, error: "boom".into() }
    }

    #[test]
    fn batch_all_pending_reports_pending_not_running() {
        // Regression test: the original aggregation OR'd Pending into the
        // same bucket as Running, making "pending" unreachable — any
        // all-Pending batch (nothing started yet) incorrectly reported
        // "running". Caught while writing this test, fixed in
        // aggregate_batch_status.
        let job_ids = vec!["a".into(), "b".into()];
        let current = vec![Some(pending_job()), Some(pending_job())];
        let (status, errored) = aggregate_batch_status(&job_ids, &current);
        assert_eq!(status, "pending");
        assert!(errored.is_empty());
    }

    #[test]
    fn batch_mix_of_pending_and_running_reports_running() {
        let job_ids = vec!["a".into(), "b".into()];
        let current = vec![Some(pending_job()), Some(running_job())];
        let (status, _) = aggregate_batch_status(&job_ids, &current);
        assert_eq!(status, "running");
    }

    #[test]
    fn batch_mix_of_pending_and_done_reports_running() {
        // One segment finished, the rest haven't started — not "pending"
        // (something has happened) and not "done" (not everything has).
        let job_ids = vec!["a".into(), "b".into()];
        let current = vec![Some(pending_job()), Some(done_job())];
        let (status, _) = aggregate_batch_status(&job_ids, &current);
        assert_eq!(status, "running");
    }

    #[test]
    fn batch_all_done_reports_done() {
        let job_ids = vec!["a".into(), "b".into()];
        let current = vec![Some(done_job()), Some(done_job())];
        let (status, errored) = aggregate_batch_status(&job_ids, &current);
        assert_eq!(status, "done");
        assert!(errored.is_empty());
    }

    #[test]
    fn batch_any_error_reports_error_with_job_ids() {
        let job_ids = vec!["a".into(), "b".into(), "c".into()];
        let current = vec![Some(done_job()), Some(error_job()), Some(running_job())];
        let (status, errored) = aggregate_batch_status(&job_ids, &current);
        assert_eq!(status, "error");
        assert_eq!(errored, vec!["b".to_string()]);
    }

    #[test]
    fn batch_missing_job_state_treated_like_pending() {
        // None (job_id not found at all, distinct from Pending) shouldn't
        // itself flip status away from "pending" if nothing else has
        // started — same bucket as Pending for aggregation purposes.
        let job_ids = vec!["a".into(), "b".into()];
        let current = vec![Some(pending_job()), None];
        let (status, _) = aggregate_batch_status(&job_ids, &current);
        assert_eq!(status, "pending");
    }

    #[test]
    fn batch_request_applies_shared_defaults() {
        let req: BatchRequest = serde_json::from_str(
            r#"{"segments": [{"text": "hi", "name": "seg01"}]}"#
        ).unwrap();
        assert_eq!(req.seed, 71463);
        assert_eq!(req.speaker, "Sarah");
        assert_eq!(req.cfg_scale, 1.3);
        assert_eq!(req.temp, None);
        assert_eq!(req.speed, None);
        assert_eq!(req.segments.len(), 1);
    }
}
