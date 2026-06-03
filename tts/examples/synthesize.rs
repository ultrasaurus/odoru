//! # synthesize
//!
//! End-to-end TTS example: text → WAV + sentence timestamps.
//!
//! Usage:
//!   source .venv/bin/activate
//!   cargo run -p tts --example synthesize

use tts::{save_wav_all, Backend, TtsEngine};
use futures::StreamExt;
use std::io;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut text = String::new();
    println!("Enter text to synthesize (then press return):");
    io::stdin().read_line(&mut text).expect("failed to read stdin");
    let text = text.trim().to_string();

    if text.is_empty() {
        anyhow::bail!("No text provided.");
    }

    let model_dir = std::env::var("KOKORO_MODEL_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            std::path::PathBuf::from(home).join(".kokoro")
        });

    println!("Initializing…");
    let engine = TtsEngine::builder()
        .backend(Backend::Kokoro {
            model_dir,
            voice: "am_puck".into(),
            speed: 1.0,
        })
        .build()?;

    println!("Synthesizing…");
    let mut stream = engine.synthesize(&text, "am_puck");
    let mut segments = Vec::new();

    while let Some(result) = stream.next().await {
        let seg = result?;
        println!("  [{:6.3}s – {:6.3}s]  {}", seg.transcript.start, seg.transcript.end, seg.transcript.text);
        segments.push(seg);
    }

    println!("{:-<50}", "");
    let total_secs = segments.last().map(|s| s.transcript.end).unwrap_or(0.0);
    println!("Total: {:.3}s ({} sentences)", total_secs, segments.len());

    save_wav_all(&segments, "output.wav", 100)?;
    println!("\nSaved: output.wav");

    Ok(())
}
