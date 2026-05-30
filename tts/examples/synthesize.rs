//! # synthesize
//!
//! End-to-end TTS example: text → WAV + sentence timestamps.
//!
//! Usage:
//!   export MISAKI_VENV=~/.misaki-g2p/venv
//!   export PYO3_PYTHON=/opt/homebrew/bin/python3.12
//!   export DYLD_LIBRARY_PATH=/opt/homebrew/Cellar/python@3.12/3.12.12/Frameworks/Python.framework/Versions/3.12/lib
//!   cargo run --example synthesize

use tts::{save_wav_all, Tts};
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

    let output_path = "output.wav";

    println!("Initializing…");
    let tts = Tts::builder().build()?;

    println!("Synthesizing…");
    let mut stream = tts.synthesize(&text);
    let mut segments = Vec::new();
    while let Some(result) = stream.next().await {
        let seg = result?;
        println!("  [{:6.3}s – {:6.3}s]  {}", seg.transcript.start, seg.transcript.end, seg.transcript.text);
        segments.push(seg);
    }

    println!("{:-<50}", "");
    let total_secs: f64 = segments.last()
        .map(|s| s.transcript.end)
        .unwrap_or(0.0);
    println!("Total: {:.3}s ({} sentences)", total_secs, segments.len());

    save_wav_all(&segments, output_path, 100)?;
    println!("\nSaved: {output_path}");

    Ok(())
}
