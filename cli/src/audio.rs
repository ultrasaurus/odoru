use indicatif::ProgressBar;
use futures::StreamExt;
use tts::{AudioSegment, TtsEngine};

pub async fn synthesize_to_mp3(
    text: &str,
    path: &str,
    engine: &TtsEngine,
    voice: &str,
    sp: &ProgressBar,
) -> anyhow::Result<()> {
    let mut all_mp3: Vec<u8> = Vec::new();
    let mut stream = engine.synthesize(text, voice);
    let mut segments_written = 0usize;

    while let Some(result) = stream.next().await {
        let seg: AudioSegment = match result {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Warning: skipping segment: {e}");
                sp.inc(1);
                continue;
            }
        };
        all_mp3.extend_from_slice(&seg.audio);
        segments_written += 1;
        sp.inc(1);
    }

    if segments_written == 0 {
        anyhow::bail!("No segments synthesized successfully");
    }

    std::fs::write(path, &all_mp3)?;
    Ok(())
}
