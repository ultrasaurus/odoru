//! # explore_model
//!
//! Phase 0: Inspect the Kokoro timestamped ONNX model.
//!
//! Runs a real inference with a short sentence and dumps:
//!   - All input/output tensor names, shapes, and dtypes
//!   - Voice file shape
//!   - Raw output values (audio samples + duration tensor)
//!
//! Usage:
//!   source .venv/bin/activate
//!   cargo run --example explore_model

use futures::StreamExt;
use ko_odoru::G2pEngine;
use ort::{inputs, session::Session, value::Tensor};
use std::path::PathBuf;

const TEST_SENTENCE: &str = "Hello world. The cat sat on the mat.";
const VOICE: &str = "af_heart";
const SAMPLE_RATE: u32 = 24_000;

fn make_session(model_path: &std::path::Path) -> anyhow::Result<Session> {
    Session::builder()
        .map_err(|e| anyhow::anyhow!("builder: {e}"))?
        .with_execution_providers([
            ort::execution_providers::CPUExecutionProvider::default().build(),
        ])
        .map_err(|e| anyhow::anyhow!("execution_providers: {e}"))?
        .commit_from_file(model_path)
        .map_err(|e| anyhow::anyhow!("commit_from_file: {e}"))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let model_dir = PathBuf::from(std::env::var("HOME").unwrap()).join(".kokoro");

    // -----------------------------------------------------------------------
    // Step 1 — Phonemize via misaki-g2p (PyO3)
    // -----------------------------------------------------------------------
    println!("━━━ Step 1: Phonemize via misaki-g2p ━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Input: {:?}", TEST_SENTENCE);

    let engine = G2pEngine::new().map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut stream = engine.phonemize(TEST_SENTENCE);

    let mut all_phonemes: Vec<(usize, String, String)> = Vec::new();
    while let Some(result) = stream.next().await {
        match result {
            Ok(chunk) => {
                println!("  [{}] {:?} => {:?}", chunk.index, chunk.sentence, chunk.phonemes);
                all_phonemes.push((chunk.index, chunk.sentence, chunk.phonemes));
            }
            Err(e) => eprintln!("  G2P error: {e}"),
        }
    }

    if all_phonemes.is_empty() {
        anyhow::bail!("No phonemes produced — check MISAKI_VENV");
    }

    // Use the first sentence for inference
    let (_, ref sentence, ref phonemes) = all_phonemes[0];
    println!("\nUsing: {:?} => {:?}", sentence, phonemes);

    // -----------------------------------------------------------------------
    // Step 2 — Tokenize phonemes
    // -----------------------------------------------------------------------
    println!("\n━━━ Step 2: Tokenize phonemes ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    let vocab = build_vocab();
    let token_ids: Vec<i64> = tokenize(phonemes, &vocab);

    println!("Phoneme chars: {}", phonemes.chars().count());
    println!("Token IDs ({} tokens): {:?}", token_ids.len(), &token_ids);

    // Add BOS=0 and EOS=0
    let mut ids_with_bos_eos = Vec::with_capacity(token_ids.len() + 2);
    ids_with_bos_eos.push(0i64);
    ids_with_bos_eos.extend_from_slice(&token_ids);
    ids_with_bos_eos.push(0i64);
    let seq_len = ids_with_bos_eos.len();
    println!("seq_len (BOS + {} tokens + EOS) = {}", token_ids.len(), seq_len);

    // -----------------------------------------------------------------------
    // Step 3 — Voice file
    // -----------------------------------------------------------------------
    println!("\n━━━ Step 3: Voice file ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    let voice_path = model_dir.join("voices").join(format!("{}.bin", VOICE));
    let voice_bytes = std::fs::read(&voice_path)?;
    let voice_floats: Vec<f32> = voice_bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();

    println!("Path:   {}", voice_path.display());
    println!("Floats: {} ({} rows of 256)", voice_floats.len(), voice_floats.len() / 256);

    let n_tokens = token_ids.len();
    let style_offset = n_tokens * 256;
    println!("Style offset: {} (n_tokens={} × 256)", style_offset, n_tokens);

    if style_offset + 256 > voice_floats.len() {
        anyhow::bail!(
            "Voice file too short: need {} floats, have {}",
            style_offset + 256,
            voice_floats.len()
        );
    }
    let style_slice: Vec<f32> = voice_floats[style_offset..style_offset + 256].to_vec();
    println!("Style first 8: {:?}", &style_slice[..8]);

    // -----------------------------------------------------------------------
    // Step 4 — Model metadata for both .onnx files
    // -----------------------------------------------------------------------
    println!("\n━━━ Step 4: Model metadata ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    ort::init()
        .with_execution_providers([
            ort::execution_providers::CPUExecutionProvider::default().build(),
        ])
        .commit();

    let meta_path = model_dir.join("model.onnx");
    println!("\n  {}", meta_path.display());
    let session = make_session(&meta_path)?;
    println!("  Inputs:");
    for input in session.inputs() {
        println!("    name={:?}  info={:?}", input.name(), input);
    }
    println!("  Outputs:");
    for output in session.outputs() {
        println!("    name={:?}  info={:?}", output.name(), output);
    }

    // -----------------------------------------------------------------------
    // Step 5 — Real inference on model.onnx
    // -----------------------------------------------------------------------
    println!("\n━━━ Step 5: Inference on model.onnx ━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    let model_path = model_dir.join("model.onnx");
    let mut session = make_session(&model_path)?;

    let input_ids_tensor = Tensor::<i64>::from_array((
        [1, seq_len],
        ids_with_bos_eos.into_boxed_slice(),
    ))
    .map_err(|e| anyhow::anyhow!("input_ids tensor: {e}"))?;

    let style_tensor = Tensor::<f32>::from_array((
        [1usize, 256],
        style_slice.into_boxed_slice(),
    ))
    .map_err(|e| anyhow::anyhow!("style tensor: {e}"))?;

    let speed_tensor = Tensor::<f32>::from_array((
        [1usize],
        vec![1.0f32].into_boxed_slice(),
    ))
    .map_err(|e| anyhow::anyhow!("speed tensor: {e}"))?;

    println!("Running inference...");
    let outputs = session
        .run(inputs![
            "input_ids" => input_ids_tensor,
            "style"     => style_tensor,
            "speed"     => speed_tensor,
        ])
        .map_err(|e| anyhow::anyhow!("inference: {e}"))?;

    println!("Output tensor count: {}", outputs.len());

    for (i, (name, value)) in outputs.iter().enumerate() {
        println!("\n  Output[{i}] name={:?}", name);

        if let Ok((shape, data)) = value.try_extract_tensor::<f32>() {
            let data: Vec<f32> = data.to_vec();
            println!("    dtype:  f32");
            println!("    shape:  {:?}", shape.to_vec());
            println!("    len:    {}", data.len());
            println!("    first10: {:?}", &data[..data.len().min(10)]);
            println!("    last10:  {:?}", &data[data.len().saturating_sub(10)..]);

            if data.len() <= seq_len + 4 {
                // Likely pred_dur
                println!("    *** SHORT — likely pred_dur, all values: {:?}", data);
                println!("    *** seq_len={seq_len}  pred_dur.len()={}", data.len());
                let raw_sum: f32 = data.iter().sum();
                println!("    *** raw sum: {:.4}", raw_sum);
                println!("    *** if half-frames (×2/80): {:.3}s", raw_sum * 2.0 / 80.0);
                println!("    *** if frames (/80):        {:.3}s", raw_sum / 80.0);
                println!("    *** if seconds (raw):       {:.3}s", raw_sum);
            } else {
                let secs = data.len() as f32 / SAMPLE_RATE as f32;
                println!("    *** LONG — likely audio: {:.3}s ({} samples @ {}Hz)",
                    secs, data.len(), SAMPLE_RATE);
                println!("    *** min={:.4}  max={:.4}",
                    data.iter().cloned().fold(f32::INFINITY, f32::min),
                    data.iter().cloned().fold(f32::NEG_INFINITY, f32::max));
            }
        } else if let Ok((shape, data)) = value.try_extract_tensor::<i64>() {
            let data: Vec<i64> = data.to_vec();
            println!("    dtype: i64  shape={:?}  len={}", shape.to_vec(), data.len());
            println!("    first10: {:?}", &data[..data.len().min(10)]);
        }
    }

    println!("\n━━━ Key questions to answer from output above ━━━━━━━━━━━━━━━━━━");
    println!("  1. pred_dur.len() == seq_len?  ({seq_len} expected)");
    println!("  2. Which duration formula matches audio length? (half-frames / frames / seconds)");
    println!("  3. Which output index is audio vs durations?");
    println!("  4. Any unexpected output tensors?");

    Ok(())
}

// ---------------------------------------------------------------------------
// Vocab helpers
// ---------------------------------------------------------------------------

fn build_vocab() -> std::collections::HashMap<char, usize> {
    let symbols = [
        '$', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'I', 'J', 'K', 'L',
        'M', 'N', 'O', 'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y',
        'Z', 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l',
        'm', 'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y',
        'z', 'ɑ', 'ɐ', 'ɒ', 'æ', 'ə', 'ɚ', 'ɛ', 'ɜ', 'ɞ', 'ɟ', 'ɡ', 'ɢ',
        'ɣ', 'ɤ', 'ɥ', 'ɦ', 'ɧ', 'ɨ', 'ɩ', 'ɪ', 'ɫ', 'ɬ', 'ɭ', 'ɮ', 'ɯ',
        'ɰ', 'ɱ', 'ɲ', 'ɳ', 'ɴ', 'ɵ', 'ɶ', 'ɷ', 'ɸ', 'ɹ', 'ɺ', 'ɻ', 'ɼ',
        'ɽ', 'ɾ', 'ɿ', 'ʀ', 'ʁ', 'ʂ', 'ʃ', 'ʄ', 'ʅ', 'ʆ', 'ʇ', 'ʈ', 'ʉ',
        'ʊ', 'ʋ', 'ʌ', 'ʍ', 'ʎ', 'ʏ', 'ʐ', 'ʑ', 'ʒ', 'ʓ', 'ʔ', 'ʕ', 'ʖ',
        'ʗ', 'ʘ', 'ʙ', 'ʚ', 'ʛ', 'ʜ', 'ʝ', 'ʞ', 'ʟ', 'ʠ', 'ʡ', 'ʢ', 'ʣ',
        'ʤ', 'ʥ', 'ʦ', 'ʧ', 'ʨ', 'ʩ', 'ʪ', 'ʫ', 'ʬ', 'ʭ', 'ʮ', 'ʯ', 'ˈ',
        'ˌ', 'ː', 'ˑ', '˒', '˓', '˔', '˕', '˖', '˗', '˘', '˙', '˚', '˛',
        '˜', '˝', '̩', 'θ', 'χ', ' ',
    ];
    symbols.iter().enumerate().map(|(i, &c)| (c, i + 1)).collect()
}

fn tokenize(phonemes: &str, vocab: &std::collections::HashMap<char, usize>) -> Vec<i64> {
    phonemes
        .chars()
        .filter_map(|c| vocab.get(&c).map(|&id| id as i64))
        .collect()
}
