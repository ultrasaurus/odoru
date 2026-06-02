//! # server
//!
//! WebSocket-based TTS server with in-memory segment cache.
//!
//! Usage:
//!   cargo run --bin server
//!
//! Protocol:
//!   client → server: { "text": "..." }
//!   server → client: { "index": 0, "transcript": {...}, "audio": "<base64 f32le PCM>", "paragraph_end": bool }
//!   server → client: { "done": true }
//!
//! Cache:
//!   Key: SHA-256(text + voice)
//!   Value: Vec<CachedSegment> — fully rendered segments ready to stream

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
use tts::{AudioSegment, Backend, TtsEngine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tower_http::{cors::CorsLayer, services::ServeDir};

// ---------------------------------------------------------------------------
// Cache
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

type Cache = Arc<DashMap<String, Vec<CachedSegment>>>;

fn cache_key(text: &str, voice: &str) -> String {
    let mut h = Sha256::new();
    h.update(text.as_bytes());
    h.update(b"|");
    h.update(voice.as_bytes());
    format!("{:x}", h.finalize())
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct AppState {
    tts: Arc<TtsEngine>,
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
}

fn default_voice() -> String { "am_puck".into() }

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

    let key = cache_key(&req.text, &req.voice);

    // ── Cache hit ────────────────────────────────────────────────────────────
    if let Some(segments) = state.cache.get(&key) {
        eprintln!("Cache hit for key {}", &key[..8]);
        for seg in segments.iter() {
            if !send_segment(&mut sender, seg, true).await {
                return;
            }
        }
        let done = serde_json::to_string(&DoneMsg { done: true }).unwrap();
        let _ = sender.send(Message::Text(done.into())).await;
        return;
    }

    // ── Cache miss: synthesize, cache, stream ────────────────────────────────
    eprintln!("Cache miss — synthesizing…");
    let mut stream = state.tts.synthesize(&req.text);
    let mut rendered: Vec<CachedSegment> = Vec::new();

    while let Some(result) = stream.next().await {
        match result {
            Ok(seg) => {
                let cached_seg = CachedSegment::from_segment(&seg);
                if !send_segment(&mut sender, &cached_seg, false).await {
                    return;
                }
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
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    eprintln!("Initializing TTS engine…");

    let model_dir = std::env::var("KOKORO_MODEL_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            std::path::PathBuf::from(home).join(".kokoro")
        });

    let tts = TtsEngine::builder()
        .backend(Backend::Kokoro {
            model_dir,
            voice: "am_puck".into(),
            speed: 1.0,
        })
        .build()?;

    eprintln!("TTS ready.");

    let state = Arc::new(AppState {
        tts: Arc::new(tts),
        cache: Arc::new(DashMap::new()),
    });

    let frontend_dir = ["app/frontend/dist", "frontend/dist", "../frontend/dist"]
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
        eprintln!("Warning: frontend/dist not found — run `cd app/frontend && npm run build`");
    }

    let addr = "0.0.0.0:3000";
    eprintln!("Listening on http://localhost:3000");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
