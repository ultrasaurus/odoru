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
    routing::{delete, get, post},
    Json, Router,
};
use dashmap::DashMap;
use futures::{SinkExt, StreamExt};
use tts::{AudioSegment, Backend, TtsEngine, Voice};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast, mpsc};
use uuid::Uuid;
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// MP3-encoded audio bytes.
    audio_bytes: Vec<u8>,
}

impl CachedSegment {
    fn from_segment(seg: &AudioSegment) -> Self {
        Self {
            index: seg.index,
            transcript_start: seg.transcript.start,
            transcript_end: seg.transcript.end,
            transcript_text: seg.transcript.text.clone(),
            paragraph_end: seg.paragraph_end,
            audio_bytes: seg.audio.clone(),
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

/// Event broadcast to WS clients watching a document.
#[derive(Clone, Debug, Serialize)]
struct DocumentStatusEvent {
    #[serde(rename = "type")]
    msg_type: &'static str,
    id: String,
    status: documents::FetchStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
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
    /// Broadcast channel for document status events.
    /// WS clients subscribe and filter by watched document IDs.
    doc_events: broadcast::Sender<DocumentStatusEvent>,
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

/// Incoming WS messages from the client.
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMsg {
    /// Synthesize text, optionally tied to a document.
    Synth(SynthRequest),
    /// Subscribe to document_status events for a document.
    Watch(WatchRequest),
    /// Cancel an active synthesis stream.
    Cancel(CancelRequest),
}

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

#[derive(Deserialize)]
struct WatchRequest {
    document_id: String,
}

#[derive(Deserialize)]
struct CancelRequest {
    stream_id: String,
}

/// Sent once before the first segment so the client can associate segments with this request.
#[derive(Serialize)]
struct SynthStartedMsg<'a> {
    #[serde(rename = "type")]
    msg_type: &'static str,
    stream_id: &'a str,
}

/// JSON header frame sent before each binary audio frame.
#[derive(Serialize)]
struct SegmentHeaderMsg<'a> {
    #[serde(rename = "type")]
    msg_type: &'static str,
    stream_id: &'a str,
    index: usize,
    transcript: TranscriptJson<'a>,
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
struct DoneMsg<'a> {
    #[serde(rename = "type")]
    msg_type: &'static str,
    stream_id: &'a str,
}

#[derive(Serialize)]
struct ErrorMsg {
    #[serde(rename = "type")]
    msg_type: &'static str,
    error: String,
}

/// Error message tied to a specific synthesis stream.
#[derive(Serialize)]
struct SynthErrorMsg<'a> {
    #[serde(rename = "type")]
    msg_type: &'static str,
    stream_id: &'a str,
    error: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn send_error(sender: &mut futures::stream::SplitSink<WebSocket, Message>, msg: &str) {
    let json = serde_json::to_string(&ErrorMsg {
        msg_type: "error",
        error: msg.to_string(),
    }).unwrap();
    let _ = sender.send(Message::Text(json.into())).await;
}


async fn send_json<T: Serialize>(
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    msg: &T,
) -> bool {
    let json = serde_json::to_string(msg).unwrap();
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
    /// Fetch from URL (mutually exclusive with content/plain_text).
    url: Option<String>,
    /// Raw markdown content (must be paired with plain_text).
    content: Option<String>,
    /// Plain text for TTS (must be paired with content).
    plain_text: Option<String>,
    /// Optional title for text documents.
    title: Option<String>,
    /// Optional source URL for text documents (provenance metadata).
    source_url: Option<String>,
}

#[derive(Serialize)]
struct CreateDocumentResponse {
    id: String,
}

async fn create_document(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateDocumentRequest>,
) -> impl IntoResponse {
    // ── Text path (content + plain_text, no URL fetch) ──────────────────────
    if body.url.is_none() {
        let (content, plain_text) = match (&body.content, &body.plain_text) {
            (Some(c), Some(p)) => (c.clone(), p.clone()),
            _ => return (StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "provide url, or both content and plain_text" }))
            ).into_response(),
        };

        let content_hash = html_content_hash(&plain_text);

        // Dedup by content_hash.
        if let Some(id) = state.doc_index.get_by_content_hash(&content_hash).await {
            return Json(CreateDocumentResponse { id }).into_response();
        }

        let title = body.title.as_deref().filter(|s| !s.trim().is_empty());
        let source_url = body.source_url.as_deref().filter(|s| !s.trim().is_empty());
        let id = match documents::create_ready(title, source_url, &content, &plain_text, &content_hash) {
            Ok(id) => id,
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("{e}") }))).into_response(),
        };

        state.doc_index.insert(&id, None, Some(&content_hash)).await;
        info!("[documents] ready (text): {id}");
        return Json(CreateDocumentResponse { id }).into_response();
    }

    // ── URL fetch path ───────────────────────────────────────────────────────
    let url = body.url.unwrap();
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return (StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "url must start with http:// or https://" }))
        ).into_response();
    }

    // Check source_url index first (fast path).
    if let Some(id) = state.doc_index.get_by_source_url(&url).await {
        return Json(CreateDocumentResponse { id }).into_response();
    }

    // Create a fetching record immediately so the client has an ID to poll.
    let id = match documents::create_fetching(Some(&url)) {
        Ok(id) => id,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e}") }))).into_response(),
    };

    // Insert into source_url index now so concurrent requests dedup correctly.
    state.doc_index.insert(&id, Some(&url), None).await;

    // Spawn blocking fetch task.
    let url = url.clone();
    let doc_id = id.clone();
    let doc_index = state.doc_index.clone();
    let doc_events = state.doc_events.clone();

    tokio::task::spawn_blocking(move || {
        let broadcast_status = |status: documents::FetchStatus, title: Option<String>, error: Option<String>| {
            let _ = doc_events.send(DocumentStatusEvent {
                msg_type: "document_status",
                id: doc_id.clone(),
                status,
                title,
                error,
            });
        };

        let html = match dl::fetch::fetch(&url) {
            Ok(h) => h,
            Err(e) => {
                error!("[documents] fetch failed for {url}: {e}");
                let _ = documents::store_error(&doc_id, &e.to_string());
                broadcast_status(documents::FetchStatus::Error, None, Some(e.to_string()));
                return;
            }
        };

        let content_hash = html_content_hash(&html);

        // Check content_hash index — catches redirects to already-cached content.
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
            let docs_dir = match documents::documents_dir() {
                Ok(d) => d,
                Err(e) => { error!("[documents] documents_dir error: {e}"); return; }
            };
            let _ = std::fs::remove_dir_all(docs_dir.join(&doc_id));
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(
                    doc_index.insert(&existing, Some(&url), None)
                )
            });
            // Notify watcher that the doc_id they were given now resolves to existing.
            let title = documents::lookup_by_id(&existing).ok().flatten().and_then(|d| d.title);
            broadcast_status(documents::FetchStatus::Ready, title, None);
            return;
        }

        let article = match dl::extract(&html, &url) {
            Ok(a) => a,
            Err(e) => {
                error!("[documents] extract failed for {url}: {e}");
                let _ = documents::store_error(&doc_id, &e.to_string());
                broadcast_status(documents::FetchStatus::Error, None, Some(e.to_string()));
                return;
            }
        };

        let title = article.title.clone();
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
            broadcast_status(documents::FetchStatus::Error, None, Some(e.to_string()));
            return;
        }

        // Update content_hash index now that we have the hash.
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(
                doc_index.insert(&doc_id, None, Some(&content_hash))
            )
        });

        info!("[documents] ready: {doc_id} ({url})");
        broadcast_status(documents::FetchStatus::Ready, title, None);
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
    /// Present when voices.json failed to parse; voices will be empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    voices_error: Option<String>,
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
                voices_error: doc.voices_error,
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
// PATCH /documents/:id — publish flag, published_voice, and content editing
// DELETE /documents/:id — cancel in-progress jobs, remove document
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct PatchDocumentBody {
    #[serde(default)]
    publish: Option<bool>,
    published_voice: Option<String>,
    /// Full markdown content (document.md body). Must be paired with plain_text.
    content: Option<String>,
    /// Plain text for TTS (document.txt). Must be paired with content.
    plain_text: Option<String>,
    /// Metadata fields — any subset may be provided.
    title: Option<String>,
    source_url: Option<String>,
    authors: Option<Vec<String>>,
    date: Option<String>,
}

async fn patch_document(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
    Json(body): Json<PatchDocumentBody>,
) -> impl IntoResponse {
    match documents::lookup_by_id(&id) {
        Ok(None) => return (StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "document not found" }))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e}") }))).into_response(),
        Ok(Some(_)) => {}
    }

    // Content edit path — requires both fields; marks voices stale.
    if body.content.is_some() || body.plain_text.is_some() {
        match (&body.content, &body.plain_text) {
            (Some(content), Some(plain_text)) => {
                let content = content.clone();
                let plain_text = plain_text.clone();
                let doc_id = id.clone();
                let voice_lock = state.voice_lock(&id);
                let _guard = voice_lock.write().await;
                let result = tokio::task::spawn_blocking(move || {
                    documents::update_content(&doc_id, &content, &plain_text)
                }).await;
                if let Err(e) | Ok(Err(e)) = result.map_err(|e| anyhow::anyhow!(e)) {
                    return (StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({ "error": format!("{e}") }))).into_response();
                }
            }
            _ => return (StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": "content and plain_text must both be provided" }))).into_response(),
        }
    }

    // Publish / published_voice path.
    if body.publish.is_some() || body.published_voice.is_some() {
        let publish = body.publish.unwrap_or(false);
        let voice = body.published_voice.clone();
        let doc_id = id.clone();
        let voice_lock = state.voice_lock(&id);
        let _guard = voice_lock.write().await;
        let result = tokio::task::spawn_blocking(move || {
            documents::update_publish(&doc_id, publish, voice.as_deref())
        }).await;
        if let Err(e) | Ok(Err(e)) = result.map_err(|e| anyhow::anyhow!(e)) {
            return (StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("{e}") }))).into_response();
        }
    }

    // Metadata edit path — title, source_url, authors, date.
    if body.title.is_some() || body.source_url.is_some() || body.authors.is_some() || body.date.is_some() {
        let title = body.title.clone();
        let source_url = body.source_url.clone();
        let authors = body.authors.clone().unwrap_or_default();
        let date = body.date.clone();
        let doc_id = id.clone();
        let result = tokio::task::spawn_blocking(move || {
            documents::update_metadata(
                &doc_id,
                title.as_deref(),
                source_url.as_deref(),
                &authors,
                date.as_deref(),
            )
        }).await;
        if let Err(e) | Ok(Err(e)) = result.map_err(|e| anyhow::anyhow!(e)) {
            return (StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("{e}") }))).into_response();
        }
    }

    StatusCode::NO_CONTENT.into_response()
}

async fn delete_document(
    Path(id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    match documents::lookup_by_id(&id) {
        Ok(None) => return (StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "document not found" }))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e}") }))).into_response(),
        Ok(Some(_)) => {}
    }

    // Delete all jobs referencing this document before removing files.
    for shared in state.jobs.all() {
        let job_id = {
            let job = shared.read().await;
            if job.document_id.as_deref() == Some(id.as_str()) {
                Some(job.id.clone())
            } else {
                None
            }
        };
        if let Some(job_id) = job_id {
            state.jobs.delete(&job_id);
        }
    }

    // Remove from in-memory indexes and flush to disk.
    state.doc_index.remove(&id).await;

    // Drop the per-document voice lock entry.
    state.voice_locks.remove(&id);

    let result = tokio::task::spawn_blocking(move || documents::delete_document(&id)).await;
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
    let voice_id = body.voice.clone();
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
        // If the job is already done and a document_id was given, make sure
        // that document's voices.json reflects the ready state — it may be a
        // freshly-created document (e.g. after deleting and re-fetching the
        // same URL) that never had its voice status written.
        {
            let job = existing.read().await;
            if matches!(job.status, JobStatus::Done) {
                if let Some(doc_id) = &body.document_id {
                    if let Ok(dir) = documents::documents_dir().map(|d| d.join(doc_id)) {
                        let lock = state.voice_lock(doc_id);
                        let _guard = lock.write().await;
                        if let Err(e) = documents::update_voice_status_in(
                            &dir, &voice_id, VoiceStatus::Ready, None, Some(&job.id),
                        ) {
                            warn!("Failed to backfill voice ready status for {doc_id}: {e}");
                        }
                    }
                }
            }
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

    // Mark voice as in_progress in voices.json if a document_id was provided.
    if let Some(doc_id) = &body.document_id {
        let job_id = shared.read().await.id.clone();
        let dir = documents::documents_dir().map(|d| d.join(doc_id));
        if let Ok(dir) = dir {
            let lock = state.voice_lock(doc_id);
            let _guard = lock.write().await;
            if let Err(e) = documents::update_voice_status_in(
                &dir, &body.voice, VoiceStatus::InProgress, None, Some(&job_id),
            ) {
                warn!("Failed to set voice in_progress for {doc_id}: {e}");
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

async fn pause_job(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    if state.jobs.pause(&id).await {
        let shared = state.jobs.get(&id).unwrap();
        let job = shared.read().await;
        Json(JobResponse::from_job(&job)).into_response()
    } else {
        (StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "job not found or not pausable" }))).into_response()
    }
}

async fn resume_job(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> impl IntoResponse {
    let Some(shared) = state.jobs.get(&id) else {
        return (StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "job not found" }))).into_response();
    };

    let (voice_id, document_id) = {
        let job = shared.read().await;
        if job.status != JobStatus::Paused {
            return (StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "job is not paused" }))).into_response();
        }
        (job.voice.clone(), job.document_id.clone())
    };
    let Some(document_id) = document_id else {
        return (StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "job has no document_id" }))).into_response();
    };

    let plain_text = match tokio::task::spawn_blocking({
        let id = document_id.clone();
        move || documents::lookup_by_id(&id)
    }).await {
        Ok(Ok(Some(d))) => d.plain_text,
        Ok(Ok(None)) => return (StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "document not found" }))).into_response(),
        _ => return (StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "failed to load document" }))).into_response(),
    };

    let (engine, voice_name) = match state.engine_for_voice(&voice_id) {
        Ok(p) => p,
        Err(e) => return (StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e }))).into_response(),
    };

    state.jobs.resume(&id).await;
    let cancel_flag = state.jobs.register_cancel_flag(&id);
    jobs::spawn_job(shared.clone(), cancel_flag, plain_text, voice_name, voice_id,
        Some(document_id), engine, state.jobs.clone());

    let job = shared.read().await;
    Json(JobResponse::from_job(&job)).into_response()
}

async fn delete_voice(
    State(state): State<Arc<AppState>>,
    axum::extract::Path((id, voice_id)): axum::extract::Path<(String, String)>,
) -> impl IntoResponse {
    let dir = match documents::documents_dir().map(|d| d.join(&id)) {
        Ok(d) => d,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e}") }))).into_response(),
    };

    let lock = state.voice_lock(&id);
    let _guard = lock.write().await;
    match documents::delete_voice_in(&dir, &voice_id) {
        Ok(None) => (StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "voice not found" }))).into_response(),
        Ok(Some(job_id)) => {
            if !job_id.is_empty() {
                state.jobs.delete(&job_id);
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e}") }))).into_response(),
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
// GET /documents/:id/annotations — read annotations
// PUT /documents/:id/annotations — replace annotations
// ---------------------------------------------------------------------------

async fn get_annotations(Path(id): Path<String>) -> impl IntoResponse {
    match documents::lookup_by_id(&id) {
        Ok(None) => return (StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "document not found" }))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e}") }))).into_response(),
        Ok(Some(_)) => {}
    }
    match tokio::task::spawn_blocking(move || documents::read_annotations(&id)).await {
        Ok(Ok(v))  => Json(v).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e}") }))).into_response(),
        Err(e)     => (StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e}") }))).into_response(),
    }
}

async fn put_annotations(
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    match documents::lookup_by_id(&id) {
        Ok(None) => return (StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "document not found" }))).into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e}") }))).into_response(),
        Ok(Some(_)) => {}
    }
    match tokio::task::spawn_blocking(move || documents::write_annotations(&id, &body)).await {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e}") }))).into_response(),
        Err(e)     => (StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("{e}") }))).into_response(),
    }
}

// ---------------------------------------------------------------------------
// GET /overrides  — list all pronunciation overrides
// POST /overrides — add or update an override
// DELETE /overrides/:word — remove an override
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct OverridesResponse {
    overrides: Vec<OverrideEntry>,
}

#[derive(Serialize, Deserialize)]
struct OverrideEntry {
    word: String,
    replacement: String,
}

#[derive(Deserialize)]
struct AddOverrideRequest {
    word: String,
    replacement: String,
}

async fn get_overrides() -> impl IntoResponse {
    let pairs = tts::f5::normalizer::list_overrides();
    let overrides = pairs.into_iter()
        .map(|(word, replacement)| OverrideEntry { word, replacement })
        .collect();
    Json(OverridesResponse { overrides })
}

async fn add_override(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AddOverrideRequest>,
) -> impl IntoResponse {
    let word = body.word.trim().to_string();
    let replacement = body.replacement.trim().to_string();

    if word.is_empty() || replacement.is_empty() {
        return StatusCode::UNPROCESSABLE_ENTITY;
    }

    tts::f5::normalizer::add_override(&word, &replacement);

    let invalidated = tokio::task::spawn_blocking({
        let word = word.clone();
        move || tts::audio_cache::invalidate_word(&word)
    }).await.unwrap_or(0);

    state.cache.clear();

    if invalidated > 0 {
        info!("override added: {word:?} → {replacement:?}; invalidated {invalidated} disk cache entries");
    } else {
        info!("override added: {word:?} → {replacement:?}");
    }

    StatusCode::NO_CONTENT
}

async fn remove_override(
    State(state): State<Arc<AppState>>,
    Path(word): Path<String>,
) -> impl IntoResponse {
    let existed = tts::f5::normalizer::remove_override(&word);

    if existed {
        let invalidated = tokio::task::spawn_blocking({
            let word = word.clone();
            move || tts::audio_cache::invalidate_word(&word)
        }).await.unwrap_or(0);

        state.cache.clear();
        info!("override removed: {word:?}; invalidated {invalidated} disk cache entries");
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
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

    // Subscribe to document status broadcast events.
    let mut events_rx = state.doc_events.subscribe();
    // Set of document IDs this connection is watching.
    let mut watched: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Channel for frames produced by the active synth task.
    let (seg_tx, mut seg_rx) = mpsc::channel::<Message>(256);
    // Cancellation flag for the active synth task; replaced on each new synth.
    let mut active_cancel: Option<Arc<AtomicBool>> = None;

    loop {
        tokio::select! {
            // ── Incoming client message ───────────────────────────────────
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<ClientMsg>(&text) {
                            Ok(ClientMsg::Watch(req)) => {
                                let doc_id = req.document_id.clone();
                                watched.insert(req.document_id);
                                // Push current status immediately so the client
                                // doesn't miss a broadcast that fired before this
                                // watch was registered.
                                if let Ok(Some(doc)) = documents::lookup_by_id(&doc_id) {
                                    if !matches!(doc.status, documents::FetchStatus::Fetching) {
                                        let evt = DocumentStatusEvent {
                                            msg_type: "document_status",
                                            id: doc_id,
                                            status: doc.status,
                                            title: doc.title,
                                            error: None,
                                        };
                                        if !send_json(&mut sender, &evt).await { return; }
                                    }
                                }
                            }
                            Ok(ClientMsg::Synth(req)) => {
                                if req.text.trim().is_empty() {
                                    send_error(&mut sender, "text must not be empty").await;
                                    continue;
                                }
                                // Cancel any in-flight stream.
                                if let Some(prev) = active_cancel.take() {
                                    prev.store(true, Ordering::Relaxed);
                                }
                                let stream_id = Uuid::new_v4().simple().to_string();
                                // Announce the stream ID before spawning so the client
                                // can register it before the first segment arrives.
                                let started = serde_json::to_string(&SynthStartedMsg {
                                    msg_type: "synth_started",
                                    stream_id: &stream_id,
                                }).unwrap();
                                if sender.send(Message::Text(started.into())).await.is_err() { return; }
                                let cancelled = Arc::new(AtomicBool::new(false));
                                active_cancel = Some(cancelled.clone());
                                tokio::spawn(handle_synth(
                                    seg_tx.clone(), cancelled, state.clone(), req, stream_id,
                                ));
                            }
                            Ok(ClientMsg::Cancel(req)) => {
                                // Only cancel if the stream_id matches to guard against stale cancels.
                                // Since we track only the latest, just cancel whatever is active.
                                debug!("Cancel request for stream {}", &req.stream_id[..8]);
                                if let Some(prev) = active_cancel.take() {
                                    prev.store(true, Ordering::Relaxed);
                                }
                            }
                            Err(e) => {
                                send_error(&mut sender, &format!("Invalid request: {e}")).await;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => return,
                    _ => {}
                }
            }

            // ── Frames from active synth task ─────────────────────────────
            Some(frame) = seg_rx.recv() => {
                if sender.send(frame).await.is_err() { return; }
            }

            // ── Outgoing document status events ───────────────────────────
            event = events_rx.recv() => {
                match event {
                    Ok(evt) if watched.contains(&evt.id) => {
                        if !send_json(&mut sender, &evt).await { return; }
                    }
                    Ok(_) => {} // not watched by this connection
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("WS event receiver lagged by {n} messages");
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        }
    }
}

/// Synthesis task — runs concurrently with the socket loop.
/// Sends frames to `seg_tx`; checks `cancelled` before each send.
async fn handle_synth(
    seg_tx: mpsc::Sender<Message>,
    cancelled: Arc<AtomicBool>,
    state: Arc<AppState>,
    req: SynthRequest,
    stream_id: String,
) {
    // Check the cancel flag and send a frame; return early on cancel or channel close.
    macro_rules! try_send {
        ($msg:expr) => {
            if cancelled.load(Ordering::Relaxed) { return; }
            if seg_tx.send($msg).await.is_err() { return; }
        }
    }
    macro_rules! json_frame {
        ($val:expr) => {
            Message::Text(serde_json::to_string(&$val).unwrap().into())
        }
    }

    // ── Resolve voice ─────────────────────────────────────────────────────
    let voice_id = match req.voice {
        Some(v) => v,
        None => match state.default_voice() {
            Some(v) => v.to_string(),
            None => {
                try_send!(json_frame!(SynthErrorMsg {
                    msg_type: "error", stream_id: &stream_id,
                    error: "no voices available".to_string(),
                }));
                return;
            }
        }
    };

    let (engine, voice_name) = match state.engine_for_voice(&voice_id) {
        Ok(pair) => pair,
        Err(e) => {
            try_send!(json_frame!(SynthErrorMsg {
                msg_type: "error", stream_id: &stream_id, error: e,
            }));
            return;
        }
    };

    let key = cache_key(&req.text, &voice_id);

    // ── Cache hit ─────────────────────────────────────────────────────────
    if let Some(segments) = state.cache.get(&key) {
        debug!("Cache hit for key {}", &key[..8]);
        for seg in segments.iter() {
            try_send!(json_frame!(SegmentHeaderMsg {
                msg_type: "segment", stream_id: &stream_id,
                index: seg.index,
                transcript: TranscriptJson {
                    start: seg.transcript_start,
                    end: seg.transcript_end,
                    text: &seg.transcript_text,
                },
                cached: true,
                paragraph_end: seg.paragraph_end,
            }));
            // O(1) clone — Bytes is reference-counted.
            try_send!(Message::Binary(seg.audio_bytes.clone()));
        }
        try_send!(json_frame!(DoneMsg { msg_type: "done", stream_id: &stream_id }));
        return;
    }

    // ── Record voice as in_progress ───────────────────────────────────────
    if let Some(doc_id) = &req.document_id {
        let lock = state.voice_lock(doc_id);
        let _guard = lock.write().await;
        let dir = documents::documents_dir().map(|d| d.join(doc_id));
        if let Ok(dir) = dir {
            if let Err(e) = documents::update_voice_status_in(
                &dir, &voice_id, VoiceStatus::InProgress, None, None,
            ) {
                warn!("Failed to set voice in_progress for {doc_id}: {e}");
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
                try_send!(json_frame!(SegmentHeaderMsg {
                    msg_type: "segment", stream_id: &stream_id,
                    index: cached_seg.index,
                    transcript: TranscriptJson {
                        start: cached_seg.transcript_start,
                        end: cached_seg.transcript_end,
                        text: &cached_seg.transcript_text,
                    },
                    cached: false,
                    paragraph_end: cached_seg.paragraph_end,
                }));
                try_send!(Message::Binary(cached_seg.audio_bytes.clone()));
                rendered.push(cached_seg);
            }
            Err(e) => {
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
                try_send!(json_frame!(SynthErrorMsg {
                    msg_type: "error", stream_id: &stream_id,
                    error: format!("Synthesis error: {e}"),
                }));
                return;
            }
        }
    }

    debug!("Caching {} segments for key {}", rendered.len(), &key[..8]);
    state.cache.insert(key, rendered);

    // ── Mark voice ready ──────────────────────────────────────────────────
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

    try_send!(json_frame!(DoneMsg { msg_type: "done", stream_id: &stream_id }));
}

// ---------------------------------------------------------------------------
// Backend construction
// ---------------------------------------------------------------------------

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

    let names = tts::kokoro::voice_names(&model_dir);
    let voice_infos: Vec<VoiceInfo> = names.iter().map(|n| VoiceInfo {
        id: format!("kokoro:{n}"),
        name: n.clone(),
        backend: "kokoro".into(),
        description: String::new(),
    }).collect();

    let default_voice = if names.iter().any(|n| n == "af_heart") {
        "af_heart".to_string()
    } else {
        names.first().cloned().unwrap_or_else(|| "af_heart".into())
    };

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

    // Broadcast channel for document status events (capacity 64 — personal tool, low volume).
    let (doc_events, _) = broadcast::channel(64);

    let state = Arc::new(AppState {
        kokoro: kokoro_engine,
        f5: f5_engine,
        voices: all_voices,
        cache: Arc::new(DashMap::new()),
        jobs: job_store,
        doc_index,
        voice_locks: Arc::new(DashMap::new()),
        doc_events,
    });

    restart_pending_jobs(state.clone());

    let frontend_dir = ["app/frontend/dist", "frontend/dist", "../frontend/dist"]
        .iter()
        .map(std::path::PathBuf::from)
        .find(|p| p.exists());

    let mut app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/documents", get(list_documents).post(create_document))
        .route("/documents/:id", get(get_document).patch(patch_document).delete(delete_document))
        .route("/documents/:id/voices/:voice_id", delete(delete_voice))
        .route("/documents/:id/annotations", get(get_annotations).put(put_annotations))
        .route("/voices", get(get_voices))
        .route("/jobs", get(list_jobs).post(create_job))
        .route("/jobs/:id", get(get_job))
        .route("/jobs/:id/pause", post(pause_job))
        .route("/jobs/:id/resume", post(resume_job))
        .route("/overrides", get(get_overrides).post(add_override))
        .route("/overrides/:word", delete(remove_override))
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
