mod audio;

use clap::{Parser, ValueEnum};
use dl::ParsedArticle;
use tts::{Backend, TtsEngine};
use util::{documents, index::html_content_hash};
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

    /// Also synthesize audio to an MP3 file
    #[arg(long)]
    audio: bool,

    /// Output path for the MP3 file, or directory to write into.
    /// Defaults to out/<name>.mp3 in the current directory.
    #[arg(short, long, value_name = "PATH")]
    output: Option<String>,

    /// TTS backend to use when --audio is set
    #[arg(long, value_enum, default_value_t = AudioBackend::Kokoro)]
    backend: AudioBackend,

    /// Voice name for F5 backend (e.g. "sarah"). Lists available voices if
    /// combined with --backend f5. Errors if used with other backends.
    #[arg(long)]
    voice: Option<String>,
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

fn build_backend(backend: AudioBackend, voice: Option<&str>) -> anyhow::Result<(Backend, String)> {
    match backend {
        AudioBackend::Kokoro => {
            if voice.is_some() {
                anyhow::bail!("--voice is only supported with --backend f5");
            }
            let model_dir = std::env::var("KOKORO_MODEL_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| {
                    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
                    std::path::PathBuf::from(home).join(".kokoro")
                });
            Ok((Backend::Kokoro { model_dir, voice: "am_puck".into(), all_voices: vec![], speed: 1.0 }, "am_puck".into()))
        }
        AudioBackend::F5 => {
            let voices_dir = util::voice::voices_dir()
                .map_err(|e| anyhow::anyhow!("Cannot find voices directory: {e}"))?;
            let all = util::voice::VoiceDef::load_all(&voices_dir)
                .map_err(|e| anyhow::anyhow!("Failed to load voices: {e}"))?;
            if all.is_empty() {
                anyhow::bail!("No voices found in {}", voices_dir.display());
            }
            let def = match voice {
                Some(name) => {
                    let available: Vec<String> = all.iter().map(|v| v.name.clone()).collect();
                    all.into_iter()
                        .find(|v| v.name == name)
                        .ok_or_else(|| anyhow::anyhow!(
                            "Voice '{}' not found. Available: {}",
                            name,
                            available.join(", ")
                        ))?
                }
                None => all.into_iter().next().unwrap(),
            };
            let name = def.name.clone();
            let tts_voice = tts::Voice::F5Tts {
                name: def.name,
                voice_ref: def.voice_ref,
                ref_text: def.ref_text,
                speed: def.speed,
                cfg_strength: def.cfg_strength,
            };
            Ok((Backend::F5Tts { voices: vec![tts_voice], workers: 1 }, name))
        }
        AudioBackend::Mock => {
            if voice.is_some() {
                anyhow::bail!("--voice is only supported with --backend f5");
            }
            Ok((Backend::Mock, "mock".into()))
        }
    }
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
        // Check document store first (scan for matching source_url).
        if !no_cache {
            let docs = documents::list_all()?;
            if let Some(hit) = docs.into_iter().find(|d| d.source_url.as_deref() == Some(input)) {
                if hit.status == documents::FetchStatus::Ready {
                    // Re-read with content.
                    if let Some(full) = documents::lookup_by_id(&hit.id)? {
                        let article = ParsedArticle {
                            url: full.source_url.unwrap_or_else(|| input.to_string()),
                            title: full.title,
                            authors: full.authors,
                            date: full.date,
                            description: full.description,
                            content: full.content,
                            plain_text: full.plain_text,
                        };
                        return Ok((article, None));
                    }
                }
            }
        }

        // Cache miss or --no-cache: fetch and store.
        let html = dl::fetch::fetch(input)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let content_hash = html_content_hash(&html);
        let article = dl::extract(&html, input)
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        let id = documents::create_fetching(Some(input))?;
        if let Err(e) = documents::store_ready(
            &id,
            Some(input),
            article.title.as_deref(),
            &article.authors,
            article.date.as_deref(),
            article.description.as_deref(),
            &article.content,
            &article.plain_text,
            &html,
            &content_hash,
        ) {
            eprintln!("Warning: failed to store document: {e}");
        }

        Ok((article, None))
    } else {
        anyhow::bail!("'{}' is not an existing file and does not look like a URL (expected http:// or https://)", input)
    }
}

/// Resolve the final MP3 output path.
///
/// - If `-o` is given and is an existing directory: write `<dir>/<stem>.mp3`
/// - If `-o` is given and is a file path: use it directly (creates parent dirs)
/// - If `-o` is absent: write to `out/<stem>.mp3` in the current directory
fn resolve_mp3_path(output: Option<&str>, stem: &str) -> anyhow::Result<std::path::PathBuf> {
    let path = match output {
        Some(o) => {
            let p = std::path::PathBuf::from(o);
            if p.is_dir() {
                p.join(format!("{stem}.mp3"))
            } else {
                p
            }
        }
        None => std::path::PathBuf::from("out").join(format!("{stem}.mp3")),
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
            .unwrap_or_else(|| mp3_filename(&article));
        let mp3_path = resolve_mp3_path(cli.output.as_deref(), &stem)?;
        let (backend, voice_name) = build_backend(cli.backend, cli.voice.as_deref())?;
        let engine = TtsEngine::builder().backend(backend).build()?;
        let total = tts::splitter::split(&article.plain_text).len();
        let pb = audio_progress(total);
        audio::synthesize_to_mp3(&article.plain_text, mp3_path.to_str().unwrap(), &engine, &voice_name, &pb).await?;
        pb.finish_with_message(format!("✔ Audio saved to {}", mp3_path.display()));
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

fn mp3_filename(article: &ParsedArticle) -> String {
    let date = article.date.as_deref().unwrap_or("undated");
    let slug = article.title.as_deref().unwrap_or("untitled");
    let slug = slug
        .to_lowercase()
        .replace(|c: char| !c.is_alphanumeric() && c != ' ', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-");
    let slug = if slug.len() > 60 { &slug[..60] } else { &slug };
    format!("{}-{}.mp3", date, slug)
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

    // ── mp3_filename ──────────────────────────────────────────────────────

    #[test]
    fn mp3_filename_basic() {
        let a = article(Some("Hello World"), Some("2024-01-15"));
        assert_eq!(mp3_filename(&a), "2024-01-15-hello-world.mp3");
    }

    #[test]
    fn mp3_filename_strips_punctuation() {
        let a = article(Some("It's a Test: Really!"), Some("2024-03-01"));
        assert_eq!(mp3_filename(&a), "2024-03-01-its-a-test-really.mp3");
    }

    #[test]
    fn mp3_filename_truncates_long_title() {
        let long = "a".repeat(80);
        let a = article(Some(&long), Some("2024-01-01"));
        let name = mp3_filename(&a);
        // date- prefix + 60 chars + .wav
        assert_eq!(name, format!("2024-01-01-{}.mp3", "a".repeat(60)));
    }

    #[test]
    fn mp3_filename_missing_title_uses_untitled() {
        let a = article(None, Some("2024-06-01"));
        assert_eq!(mp3_filename(&a), "2024-06-01-untitled.mp3");
    }

    #[test]
    fn mp3_filename_missing_date_uses_undated() {
        let a = article(Some("My Post"), None);
        assert_eq!(mp3_filename(&a), "undated-my-post.mp3");
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

    // ── resolve_mp3_path ───────────────────────────────────────────────

    #[test]
    fn resolve_mp3_path_no_output_uses_out_dir() {
        let path = resolve_mp3_path(None, "my-article").unwrap();
        assert_eq!(path, std::path::PathBuf::from("out/my-article.mp3"));
        // created the dir
        assert!(std::path::Path::new("out").is_dir());
    }

    #[test]
    fn resolve_mp3_path_explicit_file() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let out = dir.path().join("custom.mp3");
        let path = resolve_mp3_path(Some(out.to_str().unwrap()), "ignored").unwrap();
        assert_eq!(path, out);
    }

    #[test]
    fn resolve_mp3_path_directory_appends_stem() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let path = resolve_mp3_path(Some(dir.path().to_str().unwrap()), "my-stem").unwrap();
        assert_eq!(path, dir.path().join("my-stem.mp3"));
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

        let (article, wav) = load_input(&path, false).unwrap();
        assert_eq!(article.plain_text, "Hello world.");
        assert_eq!(article.content, "Hello world.");
        assert_eq!(wav, Some(stem));
    }

    #[test]
    fn load_input_unsupported_extension_errors() {
        use tempfile::NamedTempFile;
        let tmp = NamedTempFile::with_suffix(".pdf").unwrap();
        let path = tmp.path().to_str().unwrap().to_string();
        assert!(load_input(&path, false).is_err());
    }

    // ── build_backend ─────────────────────────────────────────────────────
    //
    // The Kokoro env-var tests mutate KOKORO_MODEL_DIR, which is process-global.
    // Run them with `-- --test-threads=1` or accept that they may race in
    // parallel; in practice cargo test runs them fast enough that it's fine.

    #[test]
    fn build_backend_mock_returns_mock_variant() {
        let (backend, voice_name) = build_backend(AudioBackend::Mock, None).unwrap();
        assert!(matches!(backend, tts::Backend::Mock));
        assert_eq!(voice_name, "mock");
    }

    #[test]
    fn build_backend_mock_rejects_voice_flag() {
        let result = build_backend(AudioBackend::Mock, Some("sarah"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("--voice"));
    }

    #[test]
    fn build_backend_kokoro_uses_env_var() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("KOKORO_MODEL_DIR", "/tmp/kokoro-test");
        let (backend, voice_name) = build_backend(AudioBackend::Kokoro, None).unwrap();
        std::env::remove_var("KOKORO_MODEL_DIR");
        assert_eq!(voice_name, "am_puck");
        match backend {
            tts::Backend::Kokoro { model_dir, voice, .. } => {
                assert_eq!(model_dir, std::path::PathBuf::from("/tmp/kokoro-test"));
                assert_eq!(voice, "am_puck");
            }
            _ => panic!("expected Kokoro backend"),
        }
    }

    #[test]
    fn build_backend_kokoro_rejects_voice_flag() {
        let result = build_backend(AudioBackend::Kokoro, Some("sarah"));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("--voice"));
    }

    #[test]
    fn build_backend_kokoro_falls_back_to_home_dir() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("KOKORO_MODEL_DIR");
        let (backend, _) = build_backend(AudioBackend::Kokoro, None).unwrap();
        match backend {
            tts::Backend::Kokoro { model_dir, .. } => {
                assert!(model_dir.to_string_lossy().ends_with(".kokoro"));
            }
            _ => panic!("expected Kokoro backend"),
        }
    }

    #[test]
    fn build_backend_f5_loads_sarah_voice() {
        let (backend, voice_name) = build_backend(AudioBackend::F5, None).unwrap();
        assert_eq!(voice_name, "f5-am-puck"); // first alphabetically
        let _ = backend; // just check it doesn't error
    }

    #[test]
    fn build_backend_f5_selects_named_voice() {
        let (backend, voice_name) = build_backend(AudioBackend::F5, Some("sarah")).unwrap();
        assert_eq!(voice_name, "sarah");
        match backend {
            tts::Backend::F5Tts { voices, .. } => {
                match &voices[0] {
                    tts::Voice::F5Tts { name, .. } => assert_eq!(name, "sarah"),
                    _ => panic!("expected F5Tts voice"),
                }
            }
            _ => panic!("expected F5Tts backend"),
        }
    }

    #[test]
    fn build_backend_f5_unknown_voice_errors() {
        let result = build_backend(AudioBackend::F5, Some("nobody"));
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("nobody"));
        assert!(msg.contains("Available"));
    }

    // ── integration: mock synthesis to MP3 ────────────────────────────────

    #[tokio::test]
    async fn mock_backend_synthesizes_to_mp3() {
        use futures::StreamExt;
        use tempfile::NamedTempFile;

        let engine = tts::TtsEngine::builder()
            .backend(tts::Backend::Mock)
            .build()
            .expect("engine build failed");

        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().with_extension("mp3");

        let mut all_mp3: Vec<u8> = Vec::new();
        let mut stream = engine.synthesize("Hello world. How are you?", "mock");
        while let Some(result) = stream.next().await {
            let seg = result.expect("segment failed");
            all_mp3.extend_from_slice(&seg.audio);
        }
        std::fs::write(&path, &all_mp3).unwrap();

        assert!(path.exists(), "MP3 file not created");
        assert!(path.metadata().unwrap().len() > 0, "MP3 file is empty");
    }
}
