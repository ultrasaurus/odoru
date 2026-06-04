//! # server
//!
//! WebSocket-based TTS server with in-memory segment cache.
//!
//! ## Environment variables
//!
//!   ODORU_BACKEND     — "kokoro" (default), "f5", or "both"
//!   KOKORO_MODEL_DIR  — path to Kokoro model directory (default: ~/.kokoro)
//!   VOICES_DIR        — path to voices directory (default: auto-detected)
//!   ODORU_WORKERS     — F5 worker count (default: 1)
//!
//! ## Protocol
//!
//!   client → server: { "text": "...", "voice": "f5:sarah" }
//!   server → client: { "index": 0, "transcript": {...}, "audio": "<base64 f32le PCM>", "paragraph_end": bool }
//!   server → client: { "done": true }
//!
//! Voice IDs are prefixed with the backend name, e.g. "f5:sarah" or "kokoro:am_puck".
//! Unprefixed voice names are rejected.

mod jobs;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::StatusCode,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use dashmap::DashMap;
use futures::{SinkExt, StreamExt};
use tts::{AudioSegment, Backend, TtsEngine, Voice};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tower_http::{cors::CorsLayer, services::ServeDir};
use util::{cache, voice as voice_util};

use jobs::{JobStore, JobStatus};

// ---------------------------------------------------------------------------
// Segment cache
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct CachedSegment {
    index: usize,
    transcript_start: f64,
    transcript_end: f64,
    transcript_text: String,
    paragraph_end: bool,
    audio_b64: String,
}

impl CachedSegment {
    fn from_segment(seg: &AudioSegment) -> Self {
        let mut bytes = Vec::with_capacity(seg.samples.len() * 4);
        for s in &seg.samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        Self {
            index: seg.index,
            transcript_start: seg.transcript.start,
            transcript_end: seg.transcript.end,
            transcript_text: seg.transcript.text.clone(),
            paragraph_end: seg.paragraph_end,
            audio_b64: B64.encode(&bytes),
        }
    }
}

type SegmentCache = Arc<DashMap<String, Vec<CachedSegment>>>;

/// In-memory segment cache key. Includes the full prefixed voice ID so Kokoro
/// and F5 segments for the same text are stored separately.
fn cache_key(text: &str, voice_id: &str) -> String {
    let mut h = Sha256::new();
    h.update(voice_id.as_bytes());
    h.update(b"|");
    h.update(text.as_bytes());
    format!("{:x}", h.finalize())
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

/// A voice entry for the /voices endpoint and voice picker.
#[derive(Clone, Serialize)]
struct VoiceInfo {
    /// Prefixed voice ID sent in API requests, e.g. "f5:sarah" or "kokoro:am_puck".
    id: String,
    /// Display name with backend prefix stripped, e.g. "sarah".
    name: String,
    /// Backend name, e.g. "f5" or "kokoro".
    backend: String,
    description: String,
}

#[derive(Clone, Serialize)]
struct VoicesResponse {
    voices: Vec<VoiceInfo>,
}

#[derive(Clone)]
struct AppState {
    kokoro: Option<Arc<TtsEngine>>,
    f5: Option<Arc<TtsEngine>>,
    /// Available voices in display order (f5 first, then kokoro).
    voices: Vec<VoiceInfo>,
    cache: SegmentCache,
    jobs: Arc<JobStore>,
}

impl AppState {
    /// Prefixed ID of the first available voice, used as default.
    fn default_voice(&self) -> Option<&str> {
        self.voices.first().map(|v| v.id.as_str())
    }

    /// Resolve a prefixed voice ID (e.g. "f5:sarah") to the correct engine
    /// and bare voice name. Returns an error string if the prefix is missing,
    /// the backend is unknown, or the backend is not loaded.
    fn engine_for_voice(&self, voice_id: &str) -> Result<(Arc<TtsEngine>, String), String> {
        let (backend, name) = parse_voice_id(voice_id)
            .ok_or_else(|| format!(
                "voice must include backend prefix, e.g. \"f5:sarah\" or \"kokoro:am_puck\" (got: {voice_id:?})"
            ))?;
        match backend {
            "f5" => self.f5.clone()
                .ok_or_else(|| "f5 backend not available".into())
                .map(|e| (e, name.to_string())),
            "kokoro" => self.kokoro.clone()
                .ok_or_else(|| "kokoro backend not available".into())
                .map(|e| (e, name.to_string())),
            _ => Err(format!("unknown backend: {backend:?}")),
        }
    }
}

/// Split "f5:sarah" into ("f5", "sarah"). Returns None if there is no ':'.
fn parse_voice_id(id: &str) -> Option<(&str, &str)> {
    id.split_once(':')
}

// ---------------------------------------------------------------------------
// Protocol types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SynthRequest {
    text: String,
    /// Prefixed voice ID, e.g. "f5:sarah" or "kokoro:am_puck".
    /// Unprefixed names are rejected. Defaults to the first available voice.
    voice: Option<String>,
}

#[derive(Serialize)]
struct SegmentMsg<'a> {
    index: usize,
    transcript: TranscriptJson<'a>,
    audio: &'a str,
    cached: bool,
    paragraph_end: bool,
}

#[derive(Serialize)]
struct TranscriptJson<'a> {
    start: f64,
    end: f64,
    text: &'a str,
}

#[derive(Serialize)]
struct DoneMsg { done: bool }

#[derive(Serialize)]
struct ErrorMsg { error: String }

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn send_error(sender: &mut futures::stream::SplitSink<WebSocket, Message>, msg: &str) {
    let json = serde_json::to_string(&ErrorMsg { error: msg.to_string() }).unwrap();
    let _ = sender.send(Message::Text(json.into())).await;
}

async fn send_segment(
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    seg: &CachedSegment,
    cached: bool,
) -> bool {
    let msg = SegmentMsg {
        index: seg.index,
        transcript: TranscriptJson {
            start: seg.transcript_start,
            end: seg.transcript_end,
            text: &seg.transcript_text,
        },
        audio: &seg.audio_b64,
        cached,
        paragraph_end: seg.paragraph_end,
    };
    let json = serde_json::to_string(&msg).unwrap();
    sender.send(Message::Text(json.into())).await.is_ok()
}

// ---------------------------------------------------------------------------
// GET /voices
// ---------------------------------------------------------------------------

async fn get_voices(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(VoicesResponse { voices: state.voices.clone() })
}

// ---------------------------------------------------------------------------
// GET /doc?url=<url>[&voice=<voice_id>]
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct DocQuery {
    url: String,
    /// Optional prefixed voice ID — if provided, `cached.audio` reflects whether
    /// all sentences are in the audio disk cache for that voice.
    voice: Option<String>,
}

#[derive(Serialize)]
struct CachedInfo {
    content: bool,
    /// Voice cache key (e.g. "f5:sarah:0.85:2.0") if all audio is on disk for
    /// the requested voice; `null` otherwise or if no voice was requested.
    audio: Option<String>,
}

#[derive(Serialize)]
struct DocResponse {
    url: String,
    title: Option<String>,
    authors: Vec<String>,
    date: Option<String>,
    plain_text: String,
    content: String,
    cached: CachedInfo,
}

async fn get_doc(
    State(state): State<Arc<AppState>>,
    Query(q): Query<DocQuery>,
) -> impl IntoResponse {
    if !q.url.starts_with("http://") && !q.url.starts_with("https://") {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "url must start with http:// or https://" })),
        ).into_response();
    }

    match cache::lookup(&q.url) {
        Ok(Some(hit)) => {
            let audio = check_audio(&state, &q.url, q.voice.as_deref(),
                &hit.plain_text, &hit.synthesized_voices).await;
            return Json(DocResponse {
                url: hit.url,
                title: hit.title,
                authors: hit.authors,
                date: hit.date,
                plain_text: hit.plain_text,
                content: hit.content,
                cached: CachedInfo { content: true, audio },
            }).into_response();
        }
        Ok(None) => {}
        Err(e) => eprintln!("Cache lookup error: {e}"),
    }

    let url = q.url.clone();
    let result = tokio::task::spawn_blocking(move || dl::fetch_and_extract(&url)).await;

    let article = match result {
        Ok(Ok(a)) => a,
        Ok(Err(e)) => return (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": format!("Fetch failed: {e}") })),
        ).into_response(),
        Err(e) => return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Task error: {e}") })),
        ).into_response(),
    };

    // Use q.url (the request URL) as the cache key — trafilatura's reported
    // article.url is unreliable (often returns the site root rather than the
    // article URL), which breaks cache lookup on subsequent requests.
    if let Err(e) = cache::store(
        &q.url,
        article.title.as_deref(),
        &article.authors,
        article.date.as_deref(),
        article.description.as_deref(),
        &article.content,
        &article.plain_text,
    ) {
        eprintln!("Cache store error: {e}");
    }

    // A freshly fetched article can't have any synthesized voices yet.
    let audio = check_audio(&state, &q.url, q.voice.as_deref(),
        &article.plain_text, &[]).await;
    Json(DocResponse {
        url: q.url,
        title: article.title,
        authors: article.authors,
        date: article.date,
        plain_text: article.plain_text,
        content: article.content,
        cached: CachedInfo { content: false, audio },
    }).into_response()
}

// ---------------------------------------------------------------------------
// Audio synthesis check
// ---------------------------------------------------------------------------

/// Check whether all sentences of `plain_text` are synthesized for `voice_id`.
///
/// Fast path: voice_id is in `synthesized_voices` from the article record —
/// returns the voice cache key with no I/O or Python calls.
///
/// Slow path: runs `all_audio_cached` (Python + stat calls) in spawn_blocking.
/// On success, writes the result back to the article record so future calls
/// take the fast path.
async fn check_audio(
    state: &Arc<AppState>,
    url: &str,
    voice_id: Option<&str>,
    plain_text: &str,
    synthesized_voices: &[String],
) -> Option<String> {
    let voice_id = voice_id?;
    let (engine, voice_name) = state.engine_for_voice(voice_id).ok()?;

    // Fast path — already recorded in the article store.
    if synthesized_voices.iter().any(|v| v == voice_id) {
        return engine.voice_cache_key(&voice_name);
    }

    // Slow path — check the audio store (involves PyO3 normalizer per sentence).
    let text = plain_text.to_string();
    let vname = voice_name.clone();
    let engine2 = Arc::clone(&engine);

    let all_cached = tokio::task::spawn_blocking(move || {
        engine2.all_audio_cached(&text, &vname)
    }).await.ok()??;

    if !all_cached {
        return None;
    }

    // Persist so the next GET /doc is instant.
    let url = url.to_string();
    let vid = voice_id.to_string();
    if let Err(e) = tokio::task::spawn_blocking(move || cache::mark_synthesized(&url, &vid)).await {
        eprintln!("mark_synthesized error: {e}");
    }

    engine.voice_cache_key(&voice_name)
}

// ---------------------------------------------------------------------------
// POST /jobs  — enqueue background synthesis
// GET  /jobs  — list all jobs
// GET  /jobs/:id — single job status
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateJobRequest {
    text: String,
    /// Prefixed voice ID, e.g. "f5:sarah".
    voice: String,
}

#[derive(Serialize)]
struct JobResponse {
    id: String,
    voice: String,
    text_preview: String,
    status: JobStatus,
    total_sentences: usize,
    completed_sentences: usize,
    created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl JobResponse {
    fn from_job(job: &jobs::Job) -> Self {
        Self {
            id: job.id.clone(),
            voice: job.voice.clone(),
            text_preview: job.text_preview.clone(),
            status: job.status.clone(),
            total_sentences: job.total_sentences,
            completed_sentences: job.completed_sentences,
            created_at: job.created_at.format("%Y-%m-%d %H:%M").to_string(),
            error: job.error.clone(),
        }
    }
}

async fn create_job(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateJobRequest>,
) -> impl IntoResponse {
    if body.text.trim().is_empty() {
        return (StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "text must not be empty" }))).into_response();
    }

    let (engine, voice_name) = match state.engine_for_voice(&body.voice) {
        Ok(p) => p,
        Err(e) => return (StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e }))).into_response(),
    };

    // Dedup: return existing non-terminal job for same text + voice.
    // If it's pending with no live task (e.g. after server restart), restart it.
    let text_hash = jobs::text_hash(&body.text);
    if let Some(existing) = state.jobs.find_active(&text_hash, &body.voice).await {
        let needs_task = {
            let job = existing.read().await;
            matches!(job.status, JobStatus::Pending | JobStatus::InProgress)
                && !state.jobs.has_cancel_flag(&existing.read().await.id)
        };
        if needs_task {
            let cancel_flag = state.jobs.register_cancel_flag(
                &existing.read().await.id
            );
            // Reset to pending so the task starts cleanly.
            {
                let mut job = existing.write().await;
                job.status = JobStatus::Pending;
                job.completed_sentences = 0;
                let _ = state.jobs.persist(&job);
            }
            jobs::spawn_job(existing.clone(), cancel_flag, body.text,
                voice_name, engine, state.jobs.clone());
        }
        let job = existing.read().await;
        return Json(JobResponse::from_job(&job)).into_response();
    }

    // Count sentences so the client can show progress.
    let total = tts::splitter_sentence_count(&body.text);

    let (shared, cancel_flag) = match state.jobs.create(&body.text, &body.voice, total).await {
        Ok(pair) => pair,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to create job: {e}") }))).into_response(),
    };

    jobs::spawn_job(shared.clone(), cancel_flag, body.text, voice_name, engine, state.jobs.clone());

    let job = shared.read().await;
    Json(JobResponse::from_job(&job)).into_response()
}

async fn list_jobs(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mut results = Vec::new();
    for shared in state.jobs.all() {
        let job = shared.read().await;
        results.push(JobResponse::from_job(&job));
    }
    results.sort_by(|a, b| a.id.cmp(&b.id));
    Json(results)
}

async fn get_job(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    match state.jobs.get(&id) {
        Some(shared) => {
            let job = shared.read().await;
            Json(JobResponse::from_job(&job)).into_response()
        }
        None => (StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "job not found" }))).into_response(),
    }
}

async fn cancel_job(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    if state.jobs.cancel(&id).await {
        let shared = state.jobs.get(&id).unwrap();
        let job = shared.read().await;
        Json(JobResponse::from_job(&job)).into_response()
    } else {
        (StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "job not found or already finished" }))).into_response()
    }
}

// ---------------------------------------------------------------------------
// WebSocket handler
// ---------------------------------------------------------------------------

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    let (mut sender, mut receiver) = socket.split();

    let req = loop {
        match receiver.next().await {
            Some(Ok(Message::Text(msg))) => {
                match serde_json::from_str::<SynthRequest>(&msg) {
                    Ok(req) if !req.text.trim().is_empty() => break req,
                    Ok(_) => { send_error(&mut sender, "text must not be empty").await; return; }
                    Err(e) => { send_error(&mut sender, &format!("Invalid request: {e}")).await; return; }
                }
            }
            Some(Ok(Message::Close(_))) | None => return,
            _ => continue,
        }
    };

    // Resolve voice ID — use explicit voice or fall back to first available.
    let voice_id = match req.voice {
        Some(v) => v,
        None => match state.default_voice() {
            Some(v) => v.to_string(),
            None => { send_error(&mut sender, "no voices available").await; return; }
        }
    };

    // Route to the correct engine, rejecting unprefixed or unknown voices.
    let (engine, voice_name) = match state.engine_for_voice(&voice_id) {
        Ok(pair) => pair,
        Err(e) => { send_error(&mut sender, &e).await; return; }
    };

    let key = cache_key(&req.text, &voice_id);

    // ── Cache hit ─────────────────────────────────────────────────────────
    if let Some(segments) = state.cache.get(&key) {
        eprintln!("Cache hit for key {}", &key[..8]);
        for seg in segments.iter() {
            if !send_segment(&mut sender, seg, true).await { return; }
        }
        let done = serde_json::to_string(&DoneMsg { done: true }).unwrap();
        let _ = sender.send(Message::Text(done.into())).await;
        return;
    }

    // ── Cache miss: synthesize, cache, stream ─────────────────────────────
    eprintln!("Cache miss — synthesizing with voice '{voice_id}'…");
    let mut stream = engine.synthesize(&req.text, &voice_name);
    let mut rendered: Vec<CachedSegment> = Vec::new();

    while let Some(result) = stream.next().await {
        match result {
            Ok(seg) => {
                let cached_seg = CachedSegment::from_segment(&seg);
                if !send_segment(&mut sender, &cached_seg, false).await { return; }
                rendered.push(cached_seg);
            }
            Err(e) => {
                send_error(&mut sender, &format!("Synthesis error: {e}")).await;
                return;
            }
        }
    }

    eprintln!("Caching {} segments for key {}", rendered.len(), &key[..8]);
    state.cache.insert(key, rendered);

    let done = serde_json::to_string(&DoneMsg { done: true }).unwrap();
    let _ = sender.send(Message::Text(done.into())).await;
}

// ---------------------------------------------------------------------------
// Backend construction
// ---------------------------------------------------------------------------

/// Scan `<model_dir>/voices/` for `.bin` files and return sorted voice names.
/// Falls back to ["am_puck"] if the directory can't be read.
fn kokoro_voice_names(model_dir: &std::path::Path) -> Vec<String> {
    let voices_dir = model_dir.join("voices");
    let Ok(entries) = std::fs::read_dir(&voices_dir) else {
        eprintln!("Warning: could not read {}", voices_dir.display());
        return vec!["am_puck".into()];
    };

    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            if path.extension()?.to_str()? == "bin" {
                Some(path.file_stem()?.to_str()?.to_string())
            } else {
                None
            }
        })
        .collect();

    names.sort();
    if names.is_empty() { vec!["am_puck".into()] } else { names }
}

fn build_f5() -> anyhow::Result<(TtsEngine, Vec<VoiceInfo>)> {
    let voices_dir = voice_util::voices_dir()
        .map_err(|e| anyhow::anyhow!("Cannot find voices directory: {e}"))?;
    let defs = voice_util::VoiceDef::load_all(&voices_dir)?;
    if defs.is_empty() {
        anyhow::bail!("No voices found in {}", voices_dir.display());
    }

    let workers: usize = std::env::var("ODORU_WORKERS")
        .ok().and_then(|s| s.parse().ok()).unwrap_or(1);

    let voice_infos: Vec<VoiceInfo> = defs.iter().map(|d| VoiceInfo {
        id: format!("f5:{}", d.name),
        name: d.name.clone(),
        backend: "f5".into(),
        description: d.description.clone(),
    }).collect();

    let tts_voices: Vec<Voice> = defs.into_iter().map(|d| Voice::F5Tts {
        name: d.name,
        voice_ref: d.voice_ref,
        ref_text: d.ref_text,
        speed: d.speed,
        cfg_strength: d.cfg_strength,
    }).collect();

    eprintln!("F5 backend: {} voice(s), {} worker(s)", tts_voices.len(), workers);
    for v in &voice_infos { eprintln!("  - {}", v.id); }

    let engine = TtsEngine::builder()
        .backend(Backend::F5Tts { voices: tts_voices, workers })
        .build()?;
    Ok((engine, voice_infos))
}

fn build_kokoro() -> anyhow::Result<(TtsEngine, Vec<VoiceInfo>)> {
    let model_dir = std::env::var("KOKORO_MODEL_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            std::path::PathBuf::from(home).join(".kokoro")
        });

    let names = kokoro_voice_names(&model_dir);
    let voice_infos: Vec<VoiceInfo> = names.iter().map(|n| VoiceInfo {
        id: format!("kokoro:{n}"),
        name: n.clone(),
        backend: "kokoro".into(),
        description: String::new(),
    }).collect();

    let default_voice = names.first().cloned().unwrap_or_else(|| "am_puck".into());

    eprintln!("Kokoro backend: {} voice(s) in {}", voice_infos.len(), model_dir.display());
    for v in &voice_infos { eprintln!("  - {}", v.id); }

    let engine = TtsEngine::builder()
        .backend(Backend::Kokoro { model_dir, voice: default_voice, all_voices: names, speed: 1.0 })
        .build()?;
    Ok((engine, voice_infos))
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let backend_env = std::env::var("ODORU_BACKEND").unwrap_or_else(|_| "kokoro".into());
    let want_f5     = matches!(backend_env.as_str(), "f5"     | "both");
    let want_kokoro = matches!(backend_env.as_str(), "kokoro" | "both");

    if !want_f5 && !want_kokoro {
        anyhow::bail!("ODORU_BACKEND must be \"kokoro\", \"f5\", or \"both\" (got: {backend_env:?})");
    }

    let mut all_voices: Vec<VoiceInfo> = Vec::new();
    let mut f5_engine: Option<Arc<TtsEngine>> = None;
    let mut kokoro_engine: Option<Arc<TtsEngine>> = None;

    // F5 first so its voices appear first in the list.
    if want_f5 {
        eprintln!("Initializing F5 engine…");
        let (engine, voices) = build_f5()?;
        all_voices.extend(voices);
        f5_engine = Some(Arc::new(engine));
        eprintln!("F5 ready.");
    }

    if want_kokoro {
        eprintln!("Initializing Kokoro engine…");
        let (engine, voices) = build_kokoro()?;
        all_voices.extend(voices);
        kokoro_engine = Some(Arc::new(engine));
        eprintln!("Kokoro ready.");
    }

    let job_store = Arc::new(JobStore::load()?);
    eprintln!("Jobs store loaded ({} job(s)).", job_store.all().len());

    let state = Arc::new(AppState {
        kokoro: kokoro_engine,
        f5: f5_engine,
        voices: all_voices,
        cache: Arc::new(DashMap::new()),
        jobs: job_store,
    });

    let frontend_dir = ["app/frontend/dist", "frontend/dist", "../frontend/dist"]
        .iter()
        .map(std::path::PathBuf::from)
        .find(|p| p.exists());

    let mut app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/doc", get(get_doc))
        .route("/voices", get(get_voices))
        .route("/jobs", get(list_jobs).post(create_job))
        .route("/jobs/:id", get(get_job).delete(cancel_job))
        .layer(CorsLayer::permissive())
        .with_state(state);

    if let Some(dir) = frontend_dir {
        eprintln!("Serving frontend from {}", dir.display());
        app = app.nest_service("/", ServeDir::new(dir));
    } else {
        eprintln!("Warning: frontend/dist not found — run `cd app/frontend && npm run build`");
    }

    eprintln!("Listening on http://localhost:3000");
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    axum::serve(listener, app).await?;
    Ok(())
}
