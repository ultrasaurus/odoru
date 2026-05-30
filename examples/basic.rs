use futures::StreamExt;
use ko_odoru::{G2pEngine, G2pError};
use std::io;

#[tokio::main]
async fn main() -> Result<(), G2pError> {
    // Read all of stdin as the input text.
    let mut text = String::new();
    println!("type some text and press return to get some phonemes:");
    io::stdin().read_line(&mut text).expect("failed to read stdin");
    let text = text.trim().to_string();
    println!("working...");

    // Initialise the engine (venv path from $MISAKI_VENV).
    let engine = G2pEngine::new()?;

    // Stream phoneme chunks as sentences finish.
    let mut stream = engine.phonemize(text);

    while let Some(result) = stream.next().await {
        match result {
            Ok(chunk) => println!("[{}] {} => {}", chunk.index, chunk.sentence, chunk.phonemes),
            Err(e) => eprintln!("error: {e}"),
        }
    }

    Ok(())
}
