//! # server
//!
//! WebSocket-based TTS server with in-memory segment cache.
//!
//! ## Environment variables
//!
//!   ODORU_BACKEND     — "kokoro" (default) or "f5"
//!   KOKORO_MODEL_DIR  — path to Kokoro model directory (default: ~/.kokoro)
//!   VOICES_DIR        — path to voices directory (default: auto-detected)
//!   ODORU_WORKERS     — F5 worker count (default: 1)
//!
//! ## Protocol
//!
//!   client → server: { "text": "...", "voice": "sarah" }
//!   server → client: { "index": 0, "transcript": {...}, "audio": "<base64 f32le PCM>", "paragraph_end": bool }
//!   server → client: { "done": true }

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

fn cache_key(text: &str, voice: &str) -> String {
    let mut h = Sha256::new();
    h.update(text.as_bytes());
    h.update(b"|");
    h.update(voice.as_bytes());
    format!("{:x}", h.finalize())
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

/// A voice entry for the /voices endpoint and voice picker.
#[derive(Clone, Serialize)]
struct VoiceInfo {
    name: String,
    description: String,
}

#[derive(Clone)]
struct AppState {
    tts: Arc<TtsEngine>,
    /// Available voices, in order (first = default).
    voices: Vec<VoiceInfo>,
    cache: SegmentCache,
}

impl AppState {
    fn default_voice(&self) -> &str {
        self.voices.first().map(|v| v.name.as_str()).unwrap_or("mock")
    }
}

// ---------------------------------------------------------------------------
// Protocol types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SynthRequest {
    text: String,
    /// Voice name. Defaults to the first available voice.
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
    Json(state.voices.clone())
}

// ---------------------------------------------------------------------------
// GET /doc?url=<url>
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct DocQuery {
    url: String,
}

#[derive(Serialize)]
struct DocResponse {
    url: String,
    title: Option<String>,
    authors: Vec<String>,
    date: Option<String>,
    plain_text: String,
    content: String,
    cached: bool,
}

async fn get_doc(Query(q): Query<DocQuery>) -> impl IntoResponse {
    if !q.url.starts_with("http://") && !q.url.starts_with("https://") {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "url must start with http:// or https://" })),
        ).into_response();
    }

    match cache::lookup(&q.url) {
        Ok(Some(hit)) => {
            return Json(DocResponse {
                url: hit.url,
                title: hit.title,
                authors: hit.authors,
                date: hit.date,
                plain_text: hit.plain_text,
                content: hit.content,
                cached: true,
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

    if let Err(e) = cache::store(
        &article.url,
        article.title.as_deref(),
        &article.authors,
        article.date.as_deref(),
        article.description.as_deref(),
        &article.content,
        &article.plain_text,
    ) {
        eprintln!("Cache store error: {e}");
    }

    Json(DocResponse {
        url: article.url,
        title: article.title,
        authors: article.authors,
        date: article.date,
        plain_text: article.plain_text,
        content: article.content,
        cached: false,
    }).into_response()
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

    let voice_name = req.voice.as_deref().unwrap_or_else(|| state.default_voice()).to_string();
    let key = cache_key(&req.text, &voice_name);

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
    eprintln!("Cache miss — synthesizing with voice '{voice_name}'…");
    let mut stream = state.tts.synthesize(&req.text, &voice_name);
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

/// Scan `<model_dir>/voices/` for `.bin` files and return a sorted list
/// of `VoiceInfo`. Falls back to a single `am_puck` entry if the directory
/// can't be read (e.g. model not yet downloaded).
fn kokoro_voices(model_dir: &std::path::Path) -> Vec<VoiceInfo> {
    let voices_dir = model_dir.join("voices");
    let Ok(entries) = std::fs::read_dir(&voices_dir) else {
        eprintln!("Warning: could not read {}", voices_dir.display());
        return vec![VoiceInfo { name: "am_puck".into(), description: String::new() }];
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

    if names.is_empty() {
        return vec![VoiceInfo { name: "am_puck".into(), description: String::new() }];
    }

    names.into_iter()
        .map(|name| VoiceInfo { name, description: String::new() })
        .collect()
}

// ---------------------------------------------------------------------------
// Backend construction
// ---------------------------------------------------------------------------

fn build_backend() -> anyhow::Result<(Backend, Vec<VoiceInfo>)> {
    let backend_name = std::env::var("ODORU_BACKEND").unwrap_or_else(|_| "kokoro".into());

    match backend_name.to_lowercase().as_str() {
        "f5" => {
            let voices_dir = voice_util::voices_dir()
                .map_err(|e| anyhow::anyhow!("Cannot find voices directory: {e}"))?;
            let defs = voice_util::VoiceDef::load_all(&voices_dir)?;
            if defs.is_empty() {
                anyhow::bail!("No voices found in {}", voices_dir.display());
            }

            let workers: usize = std::env::var("ODORU_WORKERS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(1);

            let voice_infos: Vec<VoiceInfo> = defs.iter().map(|d| VoiceInfo {
                name: d.name.clone(),
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
            for v in &voice_infos { eprintln!("  - {}", v.name); }

            Ok((Backend::F5Tts { voices: tts_voices, workers }, voice_infos))
        }
        "kokoro" | _ => {
            let model_dir = std::env::var("KOKORO_MODEL_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| {
                    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
                    std::path::PathBuf::from(home).join(".kokoro")
                });

            let voice_infos = kokoro_voices(&model_dir);
            let default_voice = voice_infos.first()
                .map(|v| v.name.clone())
                .unwrap_or_else(|| "am_puck".into());

            let all_voice_names: Vec<String> = voice_infos.iter().map(|v| v.name.clone()).collect();

            eprintln!("Kokoro backend: {} voice(s) in {}", voice_infos.len(), model_dir.display());
            Ok((Backend::Kokoro { model_dir, voice: default_voice, all_voices: all_voice_names, speed: 1.0 }, voice_infos))
        }
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    eprintln!("Initializing TTS engine…");

    let (backend, voice_infos) = build_backend()?;

    let tts = TtsEngine::builder().backend(backend).build()?;
    eprintln!("TTS ready.");

    let state = Arc::new(AppState {
        tts: Arc::new(tts),
        voices: voice_infos,
        cache: Arc::new(DashMap::new()),
    });

    let frontend_dir = ["app/frontend/dist", "frontend/dist", "../frontend/dist"]
        .iter()
        .map(std::path::PathBuf::from)
        .find(|p| p.exists());

    let mut app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/doc", get(get_doc))
        .route("/voices", get(get_voices))
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
