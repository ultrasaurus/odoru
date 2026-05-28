//! # server
//!
//! WebSocket-based TTS server. Accepts text, streams back audio segments.
//!
//! Usage:
//!   cargo run --bin server
//!
//! Protocol:
//!   client → server: { "text": "..." }
//!   server → client: { "index": 0, "transcript": {...}, "audio": "<base64 f32le PCM>" }
//!   server → client: { "done": true }

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
use futures::{SinkExt, StreamExt};
use ko_odoru::tts::Tts;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower_http::{cors::CorsLayer, services::ServeDir};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct AppState {
    tts: Arc<Tts>,
}

// ---------------------------------------------------------------------------
// Protocol types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SynthRequest {
    text: String,
}

#[derive(Serialize)]
struct SegmentMsg {
    index: usize,
    transcript: SegmentJson,
    audio: String, // base64 f32le PCM
}

#[derive(Serialize)]
struct SegmentJson {
    start: f64,
    end: f64,
    text: String,
}

#[derive(Serialize)]
struct DoneMsg {
    done: bool,
}

#[derive(Serialize)]
struct ErrorMsg {
    error: String,
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

    // Wait for a single text message containing the synthesis request
    let text = loop {
        match receiver.next().await {
            Some(Ok(Message::Text(msg))) => {
                match serde_json::from_str::<SynthRequest>(&msg) {
                    Ok(req) if !req.text.trim().is_empty() => break req.text,
                    Ok(_) => {
                        let _ = sender
                            .send(Message::Text(
                                serde_json::to_string(&ErrorMsg {
                                    error: "text must not be empty".into(),
                                })
                                .unwrap().into(),
                            ))
                            .await;
                        return;
                    }
                    Err(e) => {
                        let _ = sender
                            .send(Message::Text(
                                serde_json::to_string(&ErrorMsg {
                                    error: format!("Invalid request: {e}"),
                                })
                                .unwrap().into(),
                            ))
                            .await;
                        return;
                    }
                }
            }
            Some(Ok(Message::Close(_))) | None => return,
            _ => continue,
        }
    };

    // Stream segments
    let mut stream = state.tts.synthesize(&text);
    let mut index = 0usize;

    while let Some(result) = stream.next().await {
        match result {
            Ok(seg) => {
                // Encode f32 samples as little-endian bytes → base64
                let mut bytes = Vec::with_capacity(seg.samples.len() * 4);
                for s in &seg.samples {
                    bytes.extend_from_slice(&s.to_le_bytes());
                }
                let audio = B64.encode(&bytes);

                let msg = SegmentMsg {
                    index,
                    transcript: SegmentJson {
                        start: seg.transcript.start,
                        end: seg.transcript.end,
                        text: seg.transcript.text.clone(),
                    },
                    audio,
                };

                let json = serde_json::to_string(&msg).unwrap();
                if sender.send(Message::Text(json.into())).await.is_err() {
                    return; // client disconnected
                }
                index += 1;
            }
            Err(e) => {
                let _ = sender
                    .send(Message::Text(
                        serde_json::to_string(&ErrorMsg {
                            error: format!("Synthesis error: {e}"),
                        })
                        .unwrap().into(),
                    ))
                    .await;
                return;
            }
        }
    }

    // Signal completion
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

    let state = Arc::new(AppState { tts: Arc::new(tts) });

    // Serve frontend from frontend/dist if it exists
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
