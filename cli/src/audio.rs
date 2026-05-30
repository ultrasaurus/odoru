use config::{AudioConfig, silence_samples};
use hound::{WavSpec, WavWriter, SampleFormat};
use indicatif::{ProgressBar};
use tts::{AudioSegment, Tts};

pub async fn synthesize_to_wav(
    text: &str,
    path: &str,
    tts: &Tts,
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
    let mut stream = tts.synthesize(text);

    while let Some(result) = stream.next().await {
        let seg: AudioSegment = result?;
        sp.set_message(format!("Synthesizing: {}", seg.transcript.text));
        // Write the segment samples
        for sample in &seg.samples {
            writer.write_sample(*sample)?;
        }

        // Insert silence based on context
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
    }

    writer.finalize()?;
    Ok(())
}

fn is_heading_boundary(seg: &AudioSegment) -> bool {
    // A heading boundary is a paragraph end whose text looks like a heading
    seg.paragraph_end && seg.transcript.text.starts_with('#')
}