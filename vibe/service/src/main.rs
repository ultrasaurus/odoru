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
use serde::Deserialize;
use std::sync::Arc;
use tracing::{info, warn};
use uuid::Uuid;
use watchdog::ActivityTracker;

#[derive(Clone)]
struct AppState {
    secret: Option<String>,
    gpu_info: Arc<String>,
    tracker: ActivityTracker,
}

#[derive(Deserialize)]
struct SynthesizeRequest {
    text: String,
    #[serde(default = "default_seed")]
    seed: u64,
    #[serde(default = "default_speaker")]
    speaker: String,
    #[serde(default = "default_cfg_scale")]
    cfg_scale: f64,
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

    // Sanitize: only allow UUID-shaped request IDs to prevent path traversal.
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

async fn synthesize_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<SynthesizeRequest>,
) -> Response {
    if !authorized(&state, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let request_id = Uuid::new_v4().to_string();
    let txt_path = format!("/tmp/{request_id}.txt");
    let out_dir = format!("/tmp/{request_id}_out");
    let log_path = format!("/tmp/{request_id}.log");

    if let Err(e) = tokio::fs::write(&txt_path, &req.text).await {
        warn!("failed to write input text: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    if let Err(e) = tokio::fs::create_dir_all(&out_dir).await {
        warn!("failed to create output dir: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    state.tracker.touch();
    state.tracker.increment();

    // Guard ensures decrement fires even if the client drops the connection
    // and axum cancels this future mid-inference.
    struct DecrementGuard(ActivityTracker);
    impl Drop for DecrementGuard {
        fn drop(&mut self) { self.0.touch(); self.0.decrement(); }
    }
    let _guard = DecrementGuard(state.tracker.clone());

    let start = std::time::Instant::now();
    let result = run_inference(&req, &txt_path, &out_dir, &log_path, &request_id).await;

    match result {
        Err(e) => {
            warn!("inference failed: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
        Ok((wav_bytes, seed_used)) => {
            let wall = start.elapsed().as_secs_f64();
            let audio_dur = wav_duration_secs(&wav_bytes);
            let rtf = audio_dur.map(|d| wall / d);

            let mut response_headers = HeaderMap::new();
            response_headers.insert("X-Vibe-Request-Id", hv(&request_id));
            response_headers.insert("X-Vibe-Seed", hv(&seed_used.to_string()));
            response_headers.insert("X-Vibe-Gpu", hv(&state.gpu_info));
            response_headers.insert("X-Vibe-Wall-Secs", hv(&format!("{wall:.2}")));
            if let Some(d) = audio_dur {
                response_headers.insert("X-Vibe-Audio-Secs", hv(&format!("{d:.2}")));
            }
            if let Some(r) = rtf {
                response_headers.insert("X-Vibe-Rtf", hv(&format!("{r:.3}")));
            }
            response_headers.insert(
                axum::http::header::CONTENT_TYPE,
                HeaderValue::from_static("audio/wav"),
            );

            info!(
                "synthesize done: request_id={request_id} seed={seed_used} wall={wall:.1}s{}",
                rtf.map(|r| format!(" rtf={r:.3}")).unwrap_or_default()
            );

            (response_headers, Body::from(wav_bytes)).into_response()
        }
    }
}

async fn run_inference(
    req: &SynthesizeRequest,
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

    // Parse seed from log ("Seed used: 12345")
    let log_contents = tokio::fs::read_to_string(log_path).await.unwrap_or_default();
    let seed_used = log_contents
        .lines()
        .find(|l| l.contains("Seed used:"))
        .and_then(|l| l.split_whitespace().last())
        .and_then(|s| s.parse().ok())
        .unwrap_or(req.seed);

    // Find the output wav
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

fn hv(s: &str) -> HeaderValue {
    HeaderValue::from_str(s).unwrap_or_else(|_| HeaderValue::from_static("?"))
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
    };

    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/synthesize", post(synthesize_handler))
        .route("/log/:request_id", get(log_handler))
        .with_state(state);

    let addr = "0.0.0.0:3000";
    info!("vibe-service listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
