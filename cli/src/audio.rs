use config::{AudioConfig, silence_samples};
use hound::{WavSpec, WavWriter, SampleFormat};
use indicatif::ProgressBar;
use futures::StreamExt;
use tts::{AudioSegment, TtsEngine};

pub async fn synthesize_to_wav(
    text: &str,
    path: &str,
    engine: &TtsEngine,
    config: &AudioConfig,
    sp: &ProgressBar,
) -> anyhow::Result<()> {
    let spec = WavSpec {
        channels: 1,
        sample_rate: config.sample_rate,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };

    let mut writer = WavWriter::create(path, spec)?;
    let mut stream = engine.synthesize(text);
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
        for sample in &seg.samples {
            writer.write_sample(*sample)?;
        }

        let silence_ms = if is_heading_boundary(&seg) {
            config.heading_silence_ms
        } else if seg.paragraph_end {
            config.paragraph_silence_ms
        } else {
            0
        };

        for sample in silence_samples(silence_ms, config.sample_rate) {
            writer.write_sample(sample)?;
        }
        segments_written += 1;
        sp.inc(1);
    }

    writer.finalize()?;

    if segments_written == 0 {
        anyhow::bail!("No segments synthesized successfully");
    }

    Ok(())
}

fn is_heading_boundary(seg: &AudioSegment) -> bool {
    seg.paragraph_end && seg.transcript.text.starts_with('#')
}
