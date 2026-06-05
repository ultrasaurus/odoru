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
        Path, State,
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
use tokio::sync::RwLock;
use tower_http::{cors::CorsLayer, services::ServeDir};
use tracing::{debug, error, info, warn};
use util::{documents, index::{DocumentIndex, html_content_hash}, voice as voice_util};
use util::documents::VoiceStatus;

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
    /// In-memory document indexes (source_url → uuid, content_hash → uuid).
    doc_index: Arc<DocumentIndex>,
    /// Per-document RwLock for voices.json writes, keyed by document UUID.
    voice_locks: Arc<DashMap<String, Arc<RwLock<()>>>>,
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

    /// Get or create the per-document RwLock for voices.json writes.
    fn voice_lock(&self, doc_id: &str) -> Arc<RwLock<()>> {
        self.voice_locks
            .entry(doc_id.to_string())
            .or_insert_with(|| Arc::new(RwLock::new(())))
            .clone()
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
    /// UUID of the document being synthesized. When provided, voices.json is
    /// updated to `ready` with duration on completion.
    document_id: Option<String>,
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
// POST /documents — fetch-or-create, returns { id } immediately
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateDocumentRequest {
    url: String,
}

#[derive(Serialize)]
struct CreateDocumentResponse {
    id: String,
}

async fn create_document(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateDocumentRequest>,
) -> impl IntoResponse {
    if !body.url.starts_with("http://") && !body.url.starts_with("https://") {
        return (StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "url must start with http:// or https://" }))
        ).into_response();
    }

    // Check source_url index first (fast path).
    if let Some(id) = state.doc_index.get_by_source_url(&body.url).await {
        return Json(CreateDocumentResponse { id }).into_response();
    }

    // Create a fetching record immediately so the client has an ID to poll.
    let id = match documents::create_fetching(Some(&body.url)) {
        Ok(id) => id,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e}") }))).into_response(),
    };

    // Insert into source_url index now so concurrent requests dedup correctly.
    state.doc_index.insert(&id, Some(&body.url), None).await;

    // Spawn blocking fetch task.
    let url = body.url.clone();
    let doc_id = id.clone();
    let doc_index = state.doc_index.clone();

    tokio::task::spawn_blocking(move || {
        let html = match dl::fetch::fetch(&url) {
            Ok(h) => h,
            Err(e) => {
                error!("[documents] fetch failed for {url}: {e}");
                let _ = documents::store_error(&doc_id, &e.to_string());
                return;
            }
        };

        let content_hash = html_content_hash(&html);

        // Check content_hash index — catches redirects to already-cached content.
        // We need a blocking check; use a fresh tokio runtime handle.
        // Since we're in spawn_blocking, we can't .await — use block_in_place.
        let existing_id = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(
                doc_index.get_by_content_hash(&content_hash)
            )
        });

        if let Some(existing) = existing_id {
            // This URL resolves to content we already have. Remove the
            // placeholder we just created and update the source_url index
            // to point at the existing document.
            let articles_dir = match documents::documents_dir() {
                Ok(d) => d,
                Err(e) => { error!("[documents] documents_dir error: {e}"); return; }
            };
            let _ = std::fs::remove_dir_all(articles_dir.join(&doc_id));
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(
                    doc_index.insert(&existing, Some(&url), None)
                )
            });
            return;
        }

        let article = match dl::extract(&html, &url) {
            Ok(a) => a,
            Err(e) => {
                error!("[documents] extract failed for {url}: {e}");
                let _ = documents::store_error(&doc_id, &e.to_string());
                return;
            }
        };

        if let Err(e) = documents::store_ready(
            &doc_id,
            Some(&url),
            article.title.as_deref(),
            &article.authors,
            article.date.as_deref(),
            article.description.as_deref(),
            &article.content,
            &article.plain_text,
            &html,
            &content_hash,
        ) {
            error!("[documents] store_ready failed for {doc_id}: {e}");
            let _ = documents::store_error(&doc_id, &e.to_string());
            return;
        }

        // Update content_hash index now that we have the hash.
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(
                doc_index.insert(&doc_id, None, Some(&content_hash))
            )
        });

        info!("[documents] ready: {doc_id} ({url})");
    });

    Json(CreateDocumentResponse { id }).into_response()
}

// ---------------------------------------------------------------------------
// GET /documents/:id — poll for fetch status + full document
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct DocumentResponse {
    id: String,
    status: documents::FetchStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_url: Option<String>,
    title: Option<String>,
    authors: Vec<String>,
    date: Option<String>,
    description: Option<String>,
    cached_at: Option<String>,
    /// Present only when status == ready.
    content: Option<String>,
    /// Present only when status == ready.
    plain_text: Option<String>,
    publish: bool,
    voices: documents::VoicesMap,
    /// Present only when status == error.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

async fn get_document(
    State(_state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match documents::lookup_by_id(&id) {
        Ok(Some(doc)) => {
            let error = if doc.status == documents::FetchStatus::Error {
                doc.description.clone()
            } else {
                None
            };
            let (content, plain_text) = if doc.status == documents::FetchStatus::Ready {
                (Some(doc.content), Some(doc.plain_text))
            } else {
                (None, None)
            };
            Json(DocumentResponse {
                id: doc.id,
                status: doc.status,
                source_url: doc.source_url,
                title: doc.title,
                authors: doc.authors,
                date: doc.date,
                description: doc.description,
                cached_at: doc.cached_at,
                content,
                plain_text,
                publish: doc.publish,
                voices: doc.voices,
                error,
            }).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "document not found" }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e}") }))).into_response(),
    }
}

// ---------------------------------------------------------------------------
// GET /documents — list all (no content/plain_text)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct DocumentSummary {
    id: String,
    status: documents::FetchStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    authors: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cached_at: Option<String>,
    publish: bool,
    voices: documents::VoicesMap,
}

async fn list_documents() -> impl IntoResponse {
    match documents::list_all() {
        Ok(docs) => {
            let summaries: Vec<DocumentSummary> = docs.into_iter().map(|d| DocumentSummary {
                id: d.id,
                status: d.status,
                source_url: d.source_url,
                title: d.title,
                authors: d.authors,
                date: d.date,
                description: d.description,
                cached_at: d.cached_at,
                publish: d.publish,
                voices: d.voices,
            }).collect();
            Json(summaries).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e}") }))).into_response(),
    }
}

// ---------------------------------------------------------------------------
// PATCH /documents/:id — Phase 1: publish flag + published_voice
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct PatchDocumentBody {
    #[serde(default)]
    publish: Option<bool>,
    published_voice: Option<String>,
}

async fn patch_document(
    Path(id): Path<String>,
    Json(body): Json<PatchDocumentBody>,
) -> impl IntoResponse {
    // Verify document exists.
    match documents::lookup_by_id(&id) {
        Ok(None) => return (StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "document not found" }))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e}") }))).into_response(),
        Ok(Some(_)) => {}
    }

    let publish = body.publish.unwrap_or(false);
    let voice = body.published_voice.clone();
    let result = tokio::task::spawn_blocking(move || {
        documents::update_publish(&id, publish, voice.as_deref())
    }).await;

    match result {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e}") }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e}") }))).into_response(),
    }
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
    /// Document UUID. Used to update voices.json on completion.
    #[serde(default)]
    document_id: Option<String>,
}

#[derive(Serialize)]
struct JobResponse {
    id: String,
    voice: String,
    text_preview: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    document_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    article_title: Option<String>,
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
            document_id: job.document_id.clone(),
            article_title: job.article_title.clone(),
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

    // Look up article title from the document store if a document_id was provided.
    let article_title = body.document_id.as_deref()
        .and_then(|id| documents::lookup_by_id(id).ok().flatten())
        .and_then(|d| d.title);

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
            let art_id = existing.read().await.document_id.clone();
            jobs::spawn_job(existing.clone(), cancel_flag, body.text,
                voice_name, body.voice, art_id, engine, state.jobs.clone());
        }
        let job = existing.read().await;
        return Json(JobResponse::from_job(&job)).into_response();
    }

    // Count sentences so the client can show progress.
    let total = tts::splitter_sentence_count(&body.text);

    let (shared, cancel_flag) = match state.jobs.create(
        &body.text, &body.voice, total,
        body.document_id.clone(), article_title,
    ).await {
        Ok(pair) => pair,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to create job: {e}") }))).into_response(),
    };

    // Mark voice as in-progress in voices.json if a document_id was provided.
    if let Some(doc_id) = &body.document_id {
        let job_id = shared.read().await.id.clone();
        let dir = documents::documents_dir().map(|d| d.join(doc_id));
        if let Ok(dir) = dir {
            let lock = state.voice_lock(doc_id);
            let _guard = lock.write().await;
            if let Err(e) = documents::update_voice_status_in(
                &dir, &body.voice, VoiceStatus::InProgress, None, Some(&job_id),
            ) {
                warn!("Failed to set voice in-progress for {doc_id}: {e}");
            }
        }
    }

    jobs::spawn_job(shared.clone(), cancel_flag, body.text, voice_name, body.voice,
        body.document_id, engine, state.jobs.clone());

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
// Startup: restart pending jobs sequentially
// ---------------------------------------------------------------------------

/// On startup, restart any pending jobs that have an document_id.
/// Jobs are run one at a time so the server remains responsive when a client connects.
fn restart_pending_jobs(state: Arc<AppState>) {
    tokio::spawn(async move {
        let pending: Vec<_> = {
            let mut v = Vec::new();
            for shared in state.jobs.all() {
                let job = shared.read().await;
                if job.status == JobStatus::Pending && job.document_id.is_some() {
                    v.push((shared.clone(), job.voice.clone(), job.document_id.clone().unwrap()));
                }
            }
            v
        };

        if pending.is_empty() { return; }
        info!("[jobs] auto-restarting {} pending job(s)", pending.len());

        for (shared, voice_id, document_id) in pending {
            // Skip if another path (e.g. a client POST /jobs) already started it.
            if state.jobs.has_cancel_flag(&shared.read().await.id) { continue; }

            let plain_text = match tokio::task::spawn_blocking({
                let id = document_id.clone();
                move || documents::lookup_by_id(&id)
            }).await {
                Ok(Ok(Some(d))) => d.plain_text,
                Ok(Ok(None)) => {
                    warn!("[jobs] auto-restart: document not found for id {document_id}");
                    continue;
                }
                Ok(Err(e)) => {
                    error!("[jobs] auto-restart: lookup error for id {document_id}: {e}");
                    continue;
                }
                Err(e) => {
                    error!("[jobs] auto-restart: spawn_blocking error: {e}");
                    continue;
                }
            };

            let (engine, voice_name) = match state.engine_for_voice(&voice_id) {
                Ok(p) => p,
                Err(e) => {
                    error!("[jobs] auto-restart: engine error: {e}");
                    continue;
                }
            };

            let cancel_flag = state.jobs.register_cancel_flag(&shared.read().await.id);
            {
                let mut job = shared.write().await;
                job.status = JobStatus::Pending;
                job.completed_sentences = 0;
                let _ = state.jobs.persist(&job);
            }

            // Run this job to completion before starting the next.
            let handle = jobs::spawn_job(shared, cancel_flag, plain_text,
                voice_name, voice_id, Some(document_id), engine, state.jobs.clone());
            let _ = handle.await;
        }
    });
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
        debug!("Cache hit for key {}", &key[..8]);
        for seg in segments.iter() {
            if !send_segment(&mut sender, seg, true).await { return; }
        }
        let done = serde_json::to_string(&DoneMsg { done: true }).unwrap();
        let _ = sender.send(Message::Text(done.into())).await;
        return;
    }

    // If a document_id was provided, record voice as in-progress.
    if let Some(doc_id) = &req.document_id {
        let lock = state.voice_lock(doc_id);
        let _guard = lock.write().await;
        let dir = documents::documents_dir()
            .map(|d| d.join(doc_id));
        if let Ok(dir) = dir {
            if let Err(e) = documents::update_voice_status_in(
                &dir, &voice_id, VoiceStatus::InProgress, None, None,
            ) {
                warn!("Failed to set voice in-progress for {doc_id}: {e}");
            }
        }
    }

    // ── Cache miss: synthesize, cache, stream ─────────────────────────────
    info!("Cache miss — synthesizing with voice '{voice_id}'…");
    let mut stream = engine.synthesize(&req.text, &voice_name);
    let mut rendered: Vec<CachedSegment> = Vec::new();
    let mut last_end = 0.0f64;

    while let Some(result) = stream.next().await {
        match result {
            Ok(seg) => {
                last_end = seg.transcript.end;
                let cached_seg = CachedSegment::from_segment(&seg);
                if !send_segment(&mut sender, &cached_seg, false).await { return; }
                rendered.push(cached_seg);
            }
            Err(e) => {
                // Mark voice as error if we have a document_id.
                if let Some(doc_id) = &req.document_id {
                    let lock = state.voice_lock(doc_id);
                    let _guard = lock.write().await;
                    let dir = documents::documents_dir().map(|d| d.join(doc_id));
                    if let Ok(dir) = dir {
                        let _ = documents::update_voice_status_in(
                            &dir, &voice_id, VoiceStatus::Error, None, None,
                        );
                    }
                }
                send_error(&mut sender, &format!("Synthesis error: {e}")).await;
                return;
            }
        }
    }

    debug!("Caching {} segments for key {}", rendered.len(), &key[..8]);
    state.cache.insert(key, rendered);

    // Mark voice ready in voices.json if we have a document_id.
    if let Some(doc_id) = &req.document_id {
        let lock = state.voice_lock(doc_id);
        let _guard = lock.write().await;
        let dir = documents::documents_dir().map(|d| d.join(doc_id));
        if let Ok(dir) = dir {
            if let Err(e) = documents::update_voice_status_in(
                &dir, &voice_id, VoiceStatus::Ready, Some(last_end), None,
            ) {
                error!("Failed to mark voice ready for {doc_id}/{voice_id}: {e}");
            }
        }
    }

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
        warn!("Could not read voices dir: {}", voices_dir.display());
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

    info!("F5 backend: {} voice(s), {} worker(s)", tts_voices.len(), workers);
    for v in &voice_infos { info!("  - {}", v.id); }

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

    info!("Kokoro backend: {} voice(s) in {}", voice_infos.len(), model_dir.display());
    for v in &voice_infos { info!("  - {}", v.id); }

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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
        )
        .init();

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
        info!("Initializing F5 engine…");
        let (engine, voices) = build_f5()?;
        all_voices.extend(voices);
        f5_engine = Some(Arc::new(engine));
        info!("F5 ready.");
    }

    if want_kokoro {
        info!("Initializing Kokoro engine…");
        let (engine, voices) = build_kokoro()?;
        all_voices.extend(voices);
        kokoro_engine = Some(Arc::new(engine));
        info!("Kokoro ready.");
    }

    let job_store = Arc::new(JobStore::load()?);
    info!("Jobs store loaded ({} job(s)).", job_store.all().len());

    let doc_index = Arc::new(DocumentIndex::load().await?);
    info!("Document index loaded.");

    let state = Arc::new(AppState {
        kokoro: kokoro_engine,
        f5: f5_engine,
        voices: all_voices,
        cache: Arc::new(DashMap::new()),
        jobs: job_store,
        doc_index,
        voice_locks: Arc::new(DashMap::new()),
    });

    restart_pending_jobs(state.clone());

    let frontend_dir = ["app/frontend/dist", "frontend/dist", "../frontend/dist"]
        .iter()
        .map(std::path::PathBuf::from)
        .find(|p| p.exists());

    let mut app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/documents", get(list_documents).post(create_document))
        .route("/documents/:id", get(get_document).patch(patch_document))
        .route("/voices", get(get_voices))
        .route("/jobs", get(list_jobs).post(create_job))
        .route("/jobs/:id", get(get_job).delete(cancel_job))
        .layer(CorsLayer::permissive())
        .with_state(state);

    if let Some(dir) = frontend_dir {
        info!("Serving frontend from {}", dir.display());
        app = app.nest_service("/", ServeDir::new(dir));
    } else {
        warn!("frontend/dist not found — run `cd app/frontend && npm run build`");
    }

    info!("Listening on http://localhost:3000");
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    axum::serve(listener, app).await?;
    Ok(())
}
