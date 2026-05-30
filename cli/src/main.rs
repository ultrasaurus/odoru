mod audio;

use clap::{Parser, ValueEnum};
use config::AudioConfig;
use dl::{ParsedArticle, OutputFormat};
use tts::Tts;

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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let format = match cli.format {
        Format::Markdown => OutputFormat::Markdown,
        Format::Text => OutputFormat::Text,
    };

    // Run synchronous dl work in a blocking thread so it doesn't
    // conflict with the tokio runtime
    let url = cli.url.clone();
    let article = tokio::task::spawn_blocking(move || {
        dl::fetch_and_extract(&url, format)
    })
    .await??;

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
        println!("Synthesizing audio to {}...", wav_path);
        let tts = Tts::builder().build()?;
        let config = AudioConfig::default();
        audio::synthesize_to_wav(&article.content, &wav_path, &tts, &config).await?;
        eprintln!("Done.");
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