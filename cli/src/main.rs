mod audio;

use clap::{Parser, ValueEnum};
use config::AudioConfig;
use dl::ParsedArticle;
use tts::{Backend, TtsEngine};
use util::voice::VoiceDef;
use util::cache;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::Path;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "dl", about = "Fetch a URL or read a local file (.txt, .html) as markdown or plain text")]
struct Cli {
    /// URL to fetch, or path to a local .txt or .html file
    input: String,

    /// Output format
    #[arg(long, value_enum, default_value_t = Format::Markdown)]
    format: Format,

    /// Include YAML frontmatter with article metadata
    #[arg(long)]
    frontmatter: bool,

    /// Skip the article cache — always fetch and overwrite
    #[arg(long)]
    no_cache: bool,

    /// Also synthesize audio to a WAV file
    #[arg(long)]
    audio: bool,

    /// Output path for the WAV file, or directory to write into.
    /// Defaults to out/<name>.wav in the current directory.
    #[arg(short, long, value_name = "PATH")]
    output: Option<String>,

    /// TTS backend to use when --audio is set
    #[arg(long, value_enum, default_value_t = AudioBackend::Kokoro)]
    backend: AudioBackend,
}

#[derive(ValueEnum, Clone)]
enum Format {
    Markdown,
    Text,
}

#[derive(ValueEnum, Clone)]
enum AudioBackend {
    /// Kokoro ONNX (default). Requires $KOKORO_MODEL_DIR.
    Kokoro,
    /// F5-TTS MLX. Requires $F5_VOICE_REF and $F5_REF_TEXT.
    F5,
    /// Sine-wave mock (testing only, no model weights needed).
    Mock,
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

fn build_backend(backend: AudioBackend) -> anyhow::Result<Backend> {
    match backend {
        AudioBackend::Kokoro => {
            let model_dir = std::env::var("KOKORO_MODEL_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| {
                    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
                    std::path::PathBuf::from(home).join(".kokoro")
                });
            Ok(Backend::Kokoro { model_dir, voice: "am_puck".into(), speed: 1.0 })
        }
        AudioBackend::F5 => {
            let voices_dir = workspace_root().join("voices");
            let def = VoiceDef::load(&voices_dir.join("sarah"))
                .map_err(|e| anyhow::anyhow!("Failed to load voice 'sarah': {e}"))?;
            let voice = tts::Voice::F5Tts {
                name: def.name,
                voice_ref: def.voice_ref,
                ref_text: def.ref_text,
                speed: def.speed,
                cfg_strength: def.cfg_strength,
            };
            Ok(Backend::F5Tts { voices: vec![voice], workers: 1 })
        }
        AudioBackend::Mock => Ok(Backend::Mock),
    }
}

/// Resolve the workspace root from the cli crate's manifest directory.
fn workspace_root() -> std::path::PathBuf {
    // CARGO_MANIFEST_DIR is set by cargo at compile time to cli/
    // The workspace root is one level up.
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Load article content from a local file or URL.
///
/// Local files: `.txt` (read directly) or `.html` (extract via trafilatura).
/// Any other extension is an error. If the input is not an existing file path,
/// it is treated as a URL.
fn load_input(input: &str, no_cache: bool) -> anyhow::Result<(ParsedArticle, Option<String>)> {
    let path = Path::new(input);
    if path.exists() {
        let ext = path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        match ext {
            "txt" => {
                let content = std::fs::read_to_string(path)?;
                let stem = path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("output")
                    .to_string();
                let article = ParsedArticle {
                    url: input.to_string(),
                    title: None,
                    authors: vec![],
                    date: None,
                    description: None,
                    content: content.clone(),
                    plain_text: content,
                };
                Ok((article, Some(stem)))
            }
            "html" => {
                let html = std::fs::read_to_string(path)?;
                let article = dl::extract(&html, input)
                    .map_err(|e| anyhow::anyhow!("Extraction failed: {e}"))?;
                Ok((article, None))
            }
            other => anyhow::bail!(
                "Unsupported file extension '.{other}'. Only .txt and .html are supported."
            ),
        }
    } else if input.starts_with("http://") || input.starts_with("https://") {
        // Check cache first
        if !no_cache {
            if let Some(hit) = cache::lookup(input)? {
                let article = ParsedArticle {
                    url: hit.url,
                    title: hit.title,
                    authors: hit.authors,
                    date: hit.date,
                    description: hit.description,
                    content: hit.content,
                    plain_text: hit.plain_text,
                };
                return Ok((article, None));
            }
        }

        // Cache miss or --no-cache: fetch and store
        let article = dl::fetch_and_extract(input)
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        if let Err(e) = cache::store(
            &article.url,
            article.title.as_deref(),
            &article.authors,
            article.date.as_deref(),
            article.description.as_deref(),
            &article.content,
            &article.plain_text,
        ) {
            eprintln!("Warning: failed to cache article: {e}");
        }

        Ok((article, None))
    } else {
        anyhow::bail!("'{}' is not an existing file and does not look like a URL (expected http:// or https://)", input)
    }
}

/// Resolve the final WAV output path.
///
/// - If `-o` is given and is an existing directory: write `<dir>/<stem>.wav`
/// - If `-o` is given and is a file path: use it directly (creates parent dirs)
/// - If `-o` is absent: write to `out/<stem>.wav` in the current directory
fn resolve_wav_path(output: Option<&str>, stem: &str) -> anyhow::Result<std::path::PathBuf> {
    let path = match output {
        Some(o) => {
            let p = std::path::PathBuf::from(o);
            if p.is_dir() {
                p.join(format!("{stem}.wav"))
            } else {
                p
            }
        }
        None => std::path::PathBuf::from("out").join(format!("{stem}.wav")),
    };

    // Create parent directory if needed
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow::anyhow!("Could not create output directory {}: {e}", parent.display()))?;
        }
    }

    Ok(path)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let sp = spinner(&format!("Loading {}...", cli.input));
    let input = cli.input.clone();
    let input_display = cli.input.clone();
    let no_cache = cli.no_cache;
    let (article, wav_path_override) = match tokio::task::spawn_blocking(move || {
        load_input(&input, no_cache)
    }).await? {
        Err(e) => {
            sp.finish_with_message(format!("✗ {e}"));
            return Ok(());
        }
        Ok(a) => {
            let label = a.0.title.as_deref().unwrap_or(&input_display);
            sp.finish_with_message(format!("✔ {}", label));
            a
        }
    };

    if cli.frontmatter {
        print!("{}", render_frontmatter(&article));
    }
    let content = match cli.format {
        Format::Text => double_space_paragraphs(&article.plain_text),
        Format::Markdown => article.content.clone(),
    };
    print!("{}", content);

    if cli.audio {
        let stem = wav_path_override
            .unwrap_or_else(|| wav_filename(&article));
        let wav_path = resolve_wav_path(cli.output.as_deref(), &stem)?;
        let backend = build_backend(cli.backend)?;
        let engine = TtsEngine::builder().backend(backend).build()?;
        let config = AudioConfig::default();
        let total = tts::splitter::split(&article.plain_text).len();
        let pb = audio_progress(total);
        audio::synthesize_to_wav(&article.plain_text, wav_path.to_str().unwrap(), &engine, &config, &pb).await?;
        pb.finish_with_message(format!("✔ Audio saved to {}", wav_path.display()));
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
    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        out.push_str(line);
        out.push('\n');
        if i + 1 < lines.len() && !line.is_empty() && !lines[i + 1].is_empty() {
            out.push('\n');
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;
    use dl::ParsedArticle;
    use std::sync::Mutex;

    // Serialize tests that mutate process-global env vars.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn article(title: Option<&str>, date: Option<&str>) -> ParsedArticle {
        ParsedArticle {
            url: "https://example.com".into(),
            title: title.map(str::to_string),
            date: date.map(str::to_string),
            authors: vec![],
            description: None,
            content: String::new(),
            plain_text: String::new(),
        }
    }

    // ── wav_filename ──────────────────────────────────────────────────────

    #[test]
    fn wav_filename_basic() {
        let a = article(Some("Hello World"), Some("2024-01-15"));
        assert_eq!(wav_filename(&a), "2024-01-15-hello-world.wav");
    }

    #[test]
    fn wav_filename_strips_punctuation() {
        let a = article(Some("It's a Test: Really!"), Some("2024-03-01"));
        assert_eq!(wav_filename(&a), "2024-03-01-its-a-test-really.wav");
    }

    #[test]
    fn wav_filename_truncates_long_title() {
        let long = "a".repeat(80);
        let a = article(Some(&long), Some("2024-01-01"));
        let name = wav_filename(&a);
        // date- prefix + 60 chars + .wav
        assert_eq!(name, format!("2024-01-01-{}.wav", "a".repeat(60)));
    }

    #[test]
    fn wav_filename_missing_title_uses_untitled() {
        let a = article(None, Some("2024-06-01"));
        assert_eq!(wav_filename(&a), "2024-06-01-untitled.wav");
    }

    #[test]
    fn wav_filename_missing_date_uses_undated() {
        let a = article(Some("My Post"), None);
        assert_eq!(wav_filename(&a), "undated-my-post.wav");
    }

    // ── double_space_paragraphs ───────────────────────────────────────────

    #[test]
    fn double_space_paragraphs_inserts_blank_between_paragraphs() {
        let input = "First paragraph.\nSecond paragraph.";
        let output = double_space_paragraphs(input);
        assert_eq!(output, "First paragraph.\n\nSecond paragraph.\n");
    }

    #[test]
    fn double_space_paragraphs_preserves_existing_blank_lines() {
        let input = "Para one.\n\nPara two.";
        let output = double_space_paragraphs(input);
        assert!(output.contains("Para one."));
        assert!(output.contains("Para two."));
    }

    #[test]
    fn double_space_paragraphs_no_extra_space_after_blank_line() {
        let input = "Line one.\n\nLine two.";
        let output = double_space_paragraphs(input);
        assert!(!output.contains("\n\n\n"));
    }

    #[test]
    fn double_space_paragraphs_single_line_unchanged() {
        let input = "Just one line.";
        let output = double_space_paragraphs(input);
        assert_eq!(output, "Just one line.\n");
    }

    // ── resolve_wav_path ───────────────────────────────────────────────

    #[test]
    fn resolve_wav_path_no_output_uses_out_dir() {
        let path = resolve_wav_path(None, "my-article").unwrap();
        assert_eq!(path, std::path::PathBuf::from("out/my-article.wav"));
        // created the dir
        assert!(std::path::Path::new("out").is_dir());
    }

    #[test]
    fn resolve_wav_path_explicit_file() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let out = dir.path().join("custom.wav");
        let path = resolve_wav_path(Some(out.to_str().unwrap()), "ignored").unwrap();
        assert_eq!(path, out);
    }

    #[test]
    fn resolve_wav_path_directory_appends_stem() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let path = resolve_wav_path(Some(dir.path().to_str().unwrap()), "my-stem").unwrap();
        assert_eq!(path, dir.path().join("my-stem.wav"));
    }

    // ── load_input ─────────────────────────────────────────────────────

    #[test]
    fn load_input_txt_reads_content_and_sets_wav_name() {
        use tempfile::NamedTempFile;
        use std::io::Write;

        let mut tmp = NamedTempFile::with_suffix(".txt").unwrap();
        write!(tmp, "Hello world.").unwrap();
        let path = tmp.path().to_str().unwrap().to_string();
        let stem = tmp.path().file_stem().unwrap().to_str().unwrap().to_string();

        let (article, wav) = load_input(&path).unwrap();
        assert_eq!(article.plain_text, "Hello world.");
        assert_eq!(article.content, "Hello world.");
        assert_eq!(wav, Some(stem));
    }

    #[test]
    fn load_input_unsupported_extension_errors() {
        use tempfile::NamedTempFile;
        let tmp = NamedTempFile::with_suffix(".pdf").unwrap();
        let path = tmp.path().to_str().unwrap().to_string();
        assert!(load_input(&path).is_err());
    }

    // ── build_backend ─────────────────────────────────────────────────────
    //
    // The Kokoro env-var tests mutate KOKORO_MODEL_DIR, which is process-global.
    // Run them with `-- --test-threads=1` or accept that they may race in
    // parallel; in practice cargo test runs them fast enough that it's fine.

    #[test]
    fn build_backend_mock_returns_mock_variant() {
        let backend = build_backend(AudioBackend::Mock).unwrap();
        assert!(matches!(backend, tts::Backend::Mock));
    }

    #[test]
    fn build_backend_kokoro_uses_env_var() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("KOKORO_MODEL_DIR", "/tmp/kokoro-test");
        let backend = build_backend(AudioBackend::Kokoro).unwrap();
        std::env::remove_var("KOKORO_MODEL_DIR");
        match backend {
            tts::Backend::Kokoro { model_dir, voice, .. } => {
                assert_eq!(model_dir, std::path::PathBuf::from("/tmp/kokoro-test"));
                assert_eq!(voice, "am_puck");
            }
            _ => panic!("expected Kokoro backend"),
        }
    }

    #[test]
    fn build_backend_kokoro_falls_back_to_home_dir() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("KOKORO_MODEL_DIR");
        let backend = build_backend(AudioBackend::Kokoro).unwrap();
        match backend {
            tts::Backend::Kokoro { model_dir, .. } => {
                assert!(model_dir.to_string_lossy().ends_with(".kokoro"));
            }
            _ => panic!("expected Kokoro backend"),
        }
    }

    #[test]
    fn build_backend_f5_loads_sarah_voice() {
        let backend = build_backend(AudioBackend::F5).unwrap();
        match backend {
            tts::Backend::F5Tts { voices, workers } => {
                assert_eq!(workers, 1);
                assert_eq!(voices.len(), 1);
                match &voices[0] {
                    tts::Voice::F5Tts { name, speed, cfg_strength, .. } => {
                        assert_eq!(name, "sarah");
                        assert!((speed - 0.85).abs() < 0.001);
                        assert!((cfg_strength - 1.5).abs() < 0.001);
                    }
                    _ => panic!("expected F5Tts voice"),
                }
            }
            _ => panic!("expected F5Tts backend"),
        }
    }

    // ── integration: mock synthesis to WAV ────────────────────────────────

    #[tokio::test]
    async fn mock_backend_synthesizes_to_wav() {
        use futures::StreamExt;
        use hound::{WavSpec, WavWriter, SampleFormat};
        use tempfile::NamedTempFile;

        let engine = tts::TtsEngine::builder()
            .backend(tts::Backend::Mock)
            .build()
            .expect("engine build failed");

        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().with_extension("wav");

        let spec = WavSpec {
            channels: 1,
            sample_rate: 24_000,
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
        };
        let mut writer = WavWriter::create(&path, spec).unwrap();
        let mut stream = engine.synthesize("Hello world. How are you?");
        while let Some(result) = stream.next().await {
            let seg = result.expect("segment failed");
            for sample in &seg.samples {
                writer.write_sample(*sample).unwrap();
            }
        }
        writer.finalize().unwrap();

        assert!(path.exists(), "WAV file not created");
        assert!(path.metadata().unwrap().len() > 0, "WAV file is empty");
    }
}
