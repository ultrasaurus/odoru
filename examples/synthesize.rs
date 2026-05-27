//! # synthesize
//!
//! End-to-end TTS example: text → WAV + sentence timestamps.
//!
//! Usage:
//!   export MISAKI_VENV=~/.misaki-g2p/venv
//!   export PYO3_PYTHON=/opt/homebrew/bin/python3.12
//!   export DYLD_LIBRARY_PATH=/opt/homebrew/Cellar/python@3.12/3.12.12/Frameworks/Python.framework/Versions/3.12/lib
//!   cargo run --example synthesize

use ko_odoru::synth::{SynthConfig, Synthesizer};
use std::io;

fn main() -> anyhow::Result<()> {
    let mut text = String::new();
    println!("Enter text to synthesize (then press return):");
    io::stdin().read_line(&mut text).expect("failed to read stdin");
    let text = text.trim().to_string();

    if text.is_empty() {
        anyhow::bail!("No text provided.");
    }

    let model_dir = format!("{}/.kokoro", std::env::var("HOME").unwrap());
    let output_path = "output.wav";

    println!("Initializing synthesizer…");
    let mut synth = Synthesizer::new(&model_dir, None)?;

    println!("Synthesizing…");
    let config = SynthConfig::default();
    let result = synth.synthesize(&text, &config)?;

    println!("\nSentence timestamps:");
    println!("{:-<50}", "");
    for ts in &result.sentences {
        println!(
            "[{:6.3}s – {:6.3}s]  {}",
            ts.start_sec, ts.end_sec, ts.sentence
        );
    }
    println!("{:-<50}", "");
    println!(
        "Total audio: {:.3}s ({} sentences)",
        result.samples.len() as f32 / result.sample_rate as f32,
        result.sentences.len()
    );

    result.save_wav(output_path)?;
    println!("\nSaved: {output_path}");

    Ok(())
}
