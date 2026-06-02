mod audio;

use clap::{Parser, ValueEnum};
use config::AudioConfig;
use dl::{ParsedArticle, OutputFormat};
use tts::{Backend, TtsEngine};
use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "dl", about = "Download a web article as markdown or plain text")]
struct Cli {
    /// URL to fetch and extract
    url: String,

    /// Output format
    #[arg(long, value_enum, default_value_t = Format::Markdown)]
    format: Format,

    /// Include YAML frontmatter with article metadata
    #[arg(long)]
    frontmatter: bool,

    /// Also synthesize audio to a WAV file
    #[arg(long)]
    audio: bool,
}

#[derive(ValueEnum, Clone)]
enum Format {
    Markdown,
    Text,
}

fn spinner(msg: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg}")
            .unwrap(),
    );
    pb.set_message(msg.to_string());
    pb.enable_steady_tick(Duration::from_millis(80));
    pb
}

fn audio_progress(total: usize) -> ProgressBar {
    let pb = ProgressBar::new(total as u64);
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} Synthesizing {pos}/{len} sentences...")
            .unwrap(),
    );
    pb.enable_steady_tick(Duration::from_millis(80));
    pb
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let format = match cli.format {
        Format::Markdown => OutputFormat::Markdown,
        Format::Text => OutputFormat::Text,
    };

    let sp = spinner(&format!("Fetching {}...", cli.url));
    let url = cli.url.clone();
    let article = match tokio::task::spawn_blocking(move || {
        dl::fetch_and_extract(&url, format)
    }).await? {
        Err(dl::ArticleError::ExtractionFailed) => {
            sp.finish_with_message("✗ Extraction failed");
            return Ok(());
        }
        Err(e) => {
            sp.finish_with_message(format!("✗ Error: {}", e));
            return Ok(());
        }
        Ok(a) => {
            sp.finish_with_message(format!(
                "✔ {}",
                a.title.as_deref().unwrap_or("Untitled")
            ));
            a
        }
    };

    if cli.frontmatter {
        print!("{}", render_frontmatter(&article));
    }
    let content = match cli.format {
        Format::Text => double_space_paragraphs(&article.content),
        Format::Markdown => article.content.clone(),
    };
    print!("{}", content);

    if cli.audio {
        let wav_path = wav_filename(&article);

        let model_dir = std::env::var("KOKORO_MODEL_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
                std::path::PathBuf::from(home).join(".kokoro")
            });

        let engine = TtsEngine::builder()
            .backend(Backend::Kokoro {
                model_dir,
                voice: "am_puck".into(),
                speed: 1.0,
            })
            .build()?;

        let config = AudioConfig::default();
        let total = tts::splitter::split(&article.content).len();
        let pb = audio_progress(total);
        audio::synthesize_to_wav(&article.content, &wav_path, &engine, &config, &pb).await?;
        pb.finish_with_message(format!("✔ Audio saved to {}", wav_path));
    }

    Ok(())
}

fn render_frontmatter(article: &ParsedArticle) -> String {
    let mut fm = String::from("---\n");
    fm.push_str(&format!(
        "title: \"{}\"\n",
        article.title.as_deref().unwrap_or("~").replace('"', "\\\"")
    ));
    fm.push_str(&format!("url: {}\n", article.url));
    fm.push_str("authors:\n");
    if article.authors.is_empty() {
        fm.push_str("  - ~\n");
    } else {
        for author in &article.authors {
            fm.push_str(&format!("  - {}\n", author));
        }
    }
    fm.push_str(&format!(
        "date: {}\n",
        article.date.as_deref().unwrap_or("~")
    ));
    fm.push_str(&format!(
        "description: \"{}\"\n",
        article.description.as_deref().unwrap_or("~").replace('"', "\\\"")
    ));
    fm.push_str("---\n\n");
    fm
}

fn double_space_paragraphs(text: &str) -> String {
    text.lines()
        .collect::<Vec<_>>()
        .windows(2)
        .fold(String::new(), |mut acc, w| {
            acc.push_str(w[0]);
            acc.push('\n');
            if !w[0].is_empty() && !w[1].is_empty() {
                acc.push('\n');
            }
            acc
        })
}

fn wav_filename(article: &ParsedArticle) -> String {
    let date = article.date.as_deref().unwrap_or("undated");
    let slug = article.title.as_deref().unwrap_or("untitled");
    let slug = slug
        .to_lowercase()
        .replace(|c: char| !c.is_alphanumeric() && c != ' ', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-");
    let slug = if slug.len() > 60 { &slug[..60] } else { &slug };
    format!("{}-{}.wav", date, slug)
}
