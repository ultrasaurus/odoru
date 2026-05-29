//! # server
//!
//! WebSocket-based TTS server with in-memory segment cache.
//!
//! Usage:
//!   cargo run --bin server
//!
//! Protocol:
//!   client → server: { "text": "..." }
//!   server → client: { "index": 0, "transcript": {...}, "audio": "<base64 f32le PCM>" }
//!   server → client: { "done": true }
//!
//! Cache:
//!   Key: SHA-256(text + voice + speed)
//!   Value: Vec<CachedSegment> — fully rendered segments ready to stream
//!   Cache hits stream at memory speed (~0 latency), no TTS required.

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use dashmap::DashMap;
use futures::{SinkExt, StreamExt};
use ko_odoru::tts::{AudioSegment, Tts};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tower_http::{cors::CorsLayer, services::ServeDir};

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

/// A fully-rendered segment stored in the cache.
/// Samples are kept as raw f32 to avoid repeated encode/decode.
#[derive(Clone)]
struct CachedSegment {
    transcript_start: f64,
    transcript_end: f64,
    transcript_text: String,
    /// Pre-encoded base64 f32le PCM — ready to send directly.
    audio_b64: String,
}

impl CachedSegment {
    fn from_segment(seg: &AudioSegment) -> Self {
        let mut bytes = Vec::with_capacity(seg.samples.len() * 4);
        for s in &seg.samples {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        Self {
            transcript_start: seg.transcript.start,
            transcript_end: seg.transcript.end,
            transcript_text: seg.transcript.text.clone(),
            audio_b64: B64.encode(&bytes),
        }
    }
}

type Cache = Arc<DashMap<String, Vec<CachedSegment>>>;

/// Compute a cache key from the request text, voice, and speed.
fn cache_key(text: &str, voice: &str, speed: f32) -> String {
    let mut h = Sha256::new();
    h.update(text.as_bytes());
    h.update(b"|");
    h.update(voice.as_bytes());
    h.update(b"|");
    h.update(speed.to_le_bytes());
    format!("{:x}", h.finalize())
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct AppState {
    tts: Arc<Tts>,
    cache: Cache,
}

// ---------------------------------------------------------------------------
// Protocol types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SynthRequest {
    text: String,
    #[serde(default = "default_voice")]
    voice: String,
    #[serde(default = "default_speed")]
    speed: f32,
}

fn default_voice() -> String { "am_puck".into() }
fn default_speed() -> f32    { 1.0 }

#[derive(Serialize)]
struct SegmentMsg<'a> {
    index: usize,
    transcript: TranscriptJson<'a>,
    audio: &'a str,
    cached: bool,
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
    index: usize,
    seg: &CachedSegment,
    cached: bool,
) -> bool {
    let msg = SegmentMsg {
        index,
        transcript: TranscriptJson {
            start: seg.transcript_start,
            end: seg.transcript_end,
            text: &seg.transcript_text,
        },
        audio: &seg.audio_b64,
        cached,
    };
    let json = serde_json::to_string(&msg).unwrap();
    sender.send(Message::Text(json.into())).await.is_ok()
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

    // Wait for a synthesis request
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

    let key = cache_key(&req.text, &req.voice, req.speed);

    // ── Cache hit: stream pre-rendered segments immediately ──────────────────
    if let Some(segments) = state.cache.get(&key) {
        eprintln!("Cache hit for key {}", &key[..8]);
        for (i, seg) in segments.iter().enumerate() {
            if !send_segment(&mut sender, i, seg, true).await {
                return; // client disconnected
            }
        }
        let done = serde_json::to_string(&DoneMsg { done: true }).unwrap();
        let _ = sender.send(Message::Text(done.into())).await;
        return;
    }

    // ── Cache miss: synthesize, cache, and stream simultaneously ─────────────
    eprintln!("Cache miss — synthesizing…");
    let mut stream = state.tts.synthesize(&req.text);
    let mut rendered: Vec<CachedSegment> = Vec::new();
    let mut index = 0usize;

    while let Some(result) = stream.next().await {
        match result {
            Ok(seg) => {
                let cached_seg = CachedSegment::from_segment(&seg);
                if !send_segment(&mut sender, index, &cached_seg, false).await {
                    return; // client disconnected — don't cache incomplete result
                }
                rendered.push(cached_seg);
                index += 1;
            }
            Err(e) => {
                send_error(&mut sender, &format!("Synthesis error: {e}")).await;
                return; // don't cache on error
            }
        }
    }

    // Store in cache only after successful completion
    eprintln!("Caching {} segments for key {}", rendered.len(), &key[..8]);
    state.cache.insert(key, rendered);

    let done = serde_json::to_string(&DoneMsg { done: true }).unwrap();
    let _ = sender.send(Message::Text(done.into())).await;
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    eprintln!("Initializing TTS engine…");
    let tts = Tts::builder().build()?;
    eprintln!("TTS ready.");

    let state = Arc::new(AppState {
        tts: Arc::new(tts),
        cache: Arc::new(DashMap::new()),
    });

    let frontend_dir = ["frontend/dist", "../frontend/dist"]
        .iter()
        .map(std::path::PathBuf::from)
        .find(|p| p.exists());

    let mut app = Router::new()
        .route("/ws", get(ws_handler))
        .layer(CorsLayer::permissive())
        .with_state(state);

    if let Some(dir) = frontend_dir {
        eprintln!("Serving frontend from {}", dir.display());
        app = app.nest_service("/", ServeDir::new(dir));
    } else {
        eprintln!("Warning: frontend/dist not found — run `cd frontend && npm run build`");
    }

    let addr = "0.0.0.0:3000";
    eprintln!("Listening on http://localhost:3000");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
