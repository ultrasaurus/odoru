mod watchdog;

use anyhow::Result;
use axum::{
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn, error};
use uuid::Uuid;
use watchdog::ActivityTracker;

#[derive(Clone)]
struct AppState {
    secret: Option<String>,
    gpu_info: Arc<String>,
    tracker: ActivityTracker,
    jobs: Arc<RwLock<HashMap<String, JobState>>>,
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
    name: Option<String>,
}

#[derive(Serialize)]
struct JobCreated {
    job_id: String,
    name: Option<String>,
}

fn default_seed() -> u64 { 71463 }
fn default_speaker() -> String { "Sarah".into() }
fn default_cfg_scale() -> f64 { 1.3 }

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

    info!(job_id = %job_id, name = ?name, "job created");

    state.jobs.write().await.insert(job_id.clone(), JobState::Pending { name: name.clone() });

    let state2 = state.clone();
    let job_id2 = job_id.clone();
    tokio::spawn(async move {
        run_job(state2, job_id2, req).await;
    });

    Json(JobCreated { job_id, name }).into_response()
}

async fn get_job_handler(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

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

    let wav_bytes = {
        let jobs = state.jobs.read().await;
        match jobs.get(&job_id) {
            None => return StatusCode::NOT_FOUND.into_response(),
            Some(JobState::Done { wav_bytes, .. }) => wav_bytes.clone(),
            Some(other) => {
                let status_str = match other {
                    JobState::Pending { .. } => "pending",
                    JobState::Running { .. } => "running",
                    JobState::Error { .. } => "error",
                    JobState::Done { .. } => unreachable!(),
                };
                return (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({ "status": status_str })),
                )
                    .into_response();
            }
        }
    };

    // Drop wav bytes from job store after fetch; keep align data for subsequent fetches.
    {
        let mut jobs = state.jobs.write().await;
        if let Some(JobState::Done { name, seed, wall_secs, audio_secs, rtf, align, .. }) =
            jobs.remove(&job_id)
        {
            info!(job_id = %job_id, name = ?name, "wav fetched, job removed from memory");
            jobs.insert(job_id, JobState::Done {
                name,
                seed,
                wall_secs,
                audio_secs,
                rtf,
                wav_bytes: Arc::new(vec![]),
                align,
            });
        }
    }

    (
        [(axum::http::header::CONTENT_TYPE, HeaderValue::from_static("audio/wav"))],
        Body::from(wav_bytes.as_ref().clone()),
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
    let jobs = state.jobs.read().await;
    match jobs.get(&job_id) {
        Some(JobState::Done { align: Some(a), .. }) => Json(&a.transcript).into_response(),
        Some(JobState::Done { align: None, .. }) => StatusCode::NOT_FOUND.into_response(),
        Some(_) => StatusCode::CONFLICT.into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn get_job_report_handler(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let jobs = state.jobs.read().await;
    match jobs.get(&job_id) {
        Some(JobState::Done { align: Some(a), .. }) => Json(&a.report).into_response(),
        Some(JobState::Done { align: None, .. }) => StatusCode::NOT_FOUND.into_response(),
        Some(_) => StatusCode::CONFLICT.into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
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
    // Write wav to a temp file so forced_alignment::audio::load_audio can read it.
    let tmp_path = std::path::PathBuf::from(format!("/tmp/align_{}.wav", Uuid::new_v4()));
    if let Err(e) = tokio::fs::write(&tmp_path, wav_bytes).await {
        error!(name = %name, error = %e, "alignment: failed to write temp wav");
        return None;
    }

    let align_text = strip_speaker_prefixes(text);
    let result = tokio::task::spawn_blocking(move || {
        let samples = forced_alignment::audio::load_audio(&tmp_path, forced_alignment::SAMPLE_RATE)?;
        let _ = std::fs::remove_file(&tmp_path);
        forced_alignment::align(&samples, &align_text)
    })
    .await;
    match result {
        Ok(Ok((transcript, report))) => {
            info!(
                name = %name,
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

async fn run_job(state: AppState, job_id: String, req: JobRequest) {
    let name = req.name.as_deref().unwrap_or("(unnamed)");

    {
        let mut jobs = state.jobs.write().await;
        if let Some(j) = jobs.get_mut(&job_id) {
            *j = JobState::Running { name: req.name.clone() };
        }
    }

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

    let result = async {
        tokio::fs::write(&txt_path, &req.text).await?;
        tokio::fs::create_dir_all(&out_dir).await?;
        run_inference_inner(&req, &txt_path, &out_dir, &log_path, &request_id).await
    }
    .await;

    match result {
        Err(e) => {
            warn!(job_id = %job_id, name = %name, error = %e, "job failed");
            let mut jobs = state.jobs.write().await;
            if let Some(j) = jobs.get_mut(&job_id) {
                *j = JobState::Error { name: req.name.clone(), error: e.to_string() };
            }
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
            }
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

    let mut child = tokio::process::Command::new("python3")
        .args([
            "/workspace/VibeVoice/demo/inference_from_file.py",
            "--model_path", "vibevoice/VibeVoice-1.5B",
            "--txt_path", txt_path,
            "--speaker_names", &req.speaker,
            "--cfg_scale", &req.cfg_scale.to_string(),
            "--seed", &req.seed.to_string(),
            "--output_dir", out_dir,
        ])
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

    let state = AppState {
        secret,
        gpu_info: Arc::new(gpu_info),
        tracker,
        jobs: Arc::new(RwLock::new(HashMap::new())),
    };

    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/jobs", post(create_job_handler))
        .route("/jobs/:job_id", get(get_job_handler))
        .route("/jobs/:job_id/wav", get(get_job_wav_handler))
        .route("/jobs/:job_id/transcript", get(get_job_transcript_handler))
        .route("/jobs/:job_id/report", get(get_job_report_handler))
        .route("/log/:request_id", get(log_handler))
        .with_state(state);

    let addr = "0.0.0.0:3000";
    info!("vibe-service listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
