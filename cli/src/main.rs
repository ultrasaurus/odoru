mod audio;
mod import;

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};
use dl::ParsedArticle;
use tts::{Backend, TtsEngine};
use util::{documents, index::html_content_hash};
use indicatif::{ProgressBar, ProgressStyle};
use std::path::Path;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "dl")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Fetch a URL or local file (.txt, .html) as markdown or plain text,
    /// optionally synthesizing audio (--audio, --backend, --voice, -o)
    Fetch(FetchArgs),
    /// Export published documents as a standalone static SPA
    Spa(SpaArgs),
    /// Import content from various sources into Odoru's document/audio store
    Import(ImportArgs),
    /// Write a document's current text to a file (see dev/cli-import.md)
    Export(ExportArgs),
    /// Read a file back into a document, replacing its content/plain_text
    /// (see dev/cli-import.md)
    Edit(EditArgs),
}

#[derive(Parser)]
struct ExportArgs {
    /// Document id to export
    doc_id: String,
    /// File path to write
    path: std::path::PathBuf,
    /// Which text to export: plain_text (default) or markdown content
    #[arg(long, value_enum, default_value_t = Format::Text)]
    format: Format,
}

#[derive(Parser)]
struct EditArgs {
    /// Document id to update
    doc_id: String,
    /// File path to read
    path: std::path::PathBuf,
    /// Whether the file is plain text (default) or markdown
    #[arg(long, value_enum, default_value_t = Format::Text)]
    format: Format,
}

#[derive(Parser)]
struct ImportArgs {
    #[command(subcommand)]
    command: ImportCommand,
}

#[derive(Subcommand)]
enum ImportCommand {
    /// Fetch a URL into Odoru's document store, printing the resulting
    /// document id (see dev/cli-import.md)
    Fetch(ImportFetchArgs),
    /// Import vibe-synthesized segment output into Odoru's document/audio
    /// store (see dev/tts-backends/vibe-import.md)
    Vibe(ImportVibeArgs),
}

#[derive(Parser)]
struct ImportVibeArgs {
    /// Directory containing the vibe run's <name>.segments.json sidecar
    /// and per-segment output files
    basedir: std::path::PathBuf,
}

#[derive(Parser)]
struct ImportFetchArgs {
    /// URL to fetch
    url: String,

    /// Skip the document cache — always fetch and overwrite
    #[arg(long)]
    no_cache: bool,

    /// Print the result as JSON instead of human-readable text
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
struct FetchArgs {
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

    /// Also synthesize audio and write it to an MP3 file (see --output, --backend, --voice)
    #[arg(long)]
    audio: bool,

    /// Output path for the MP3 file, or directory to write into. Only used
    /// with --audio. Defaults to out/<name>.mp3 in the current directory.
    #[arg(short, long, value_name = "PATH")]
    output: Option<String>,

    /// TTS backend to use when --audio is set
    #[arg(long, value_enum, default_value_t = AudioBackend::Kokoro)]
    backend: AudioBackend,

    /// Voice name to use when --audio is set. For --backend kokoro, this is
    /// a Kokoro preset name (e.g. "af_heart") found in $KOKORO_MODEL_DIR/voices/
    /// (default: af_heart, or the first available voice). For --backend f5,
    /// this is a name from the local voices/ directory (default: first
    /// alphabetically). Errors if the named voice isn't found, listing the
    /// voices that are available. Not supported with --backend mock.
    #[arg(long)]
    voice: Option<String>,
}

#[derive(Parser)]
struct SpaArgs {
    /// Directory to write the exported SPA into (created if it doesn't exist)
    output_dir: String,
}

#[derive(ValueEnum, Clone)]
enum Format {
    Markdown,
    Text,
}

#[derive(ValueEnum, Clone)]
enum AudioBackend {
    /// Kokoro ONNX (default). Loads voices from $KOKORO_MODEL_DIR/voices/
    /// (default: ~/.kokoro/voices/).
    Kokoro,
    /// F5-TTS MLX. Loads voice definitions from the local voices/ directory.
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
            let model_dir = std::env::var("KOKORO_MODEL_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| {
                    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
                    std::path::PathBuf::from(home).join(".kokoro")
                });
            let names = tts::kokoro::voice_names(&model_dir);
            let chosen = match voice {
                Some(name) => {
                    if !names.iter().any(|n| n == name) {
                        anyhow::bail!(
                            "Voice '{}' not found. Available: {}",
                            name,
                            names.join(", ")
                        );
                    }
                    name.to_string()
                }
                None => {
                    if names.iter().any(|n| n == "af_heart") {
                        "af_heart".to_string()
                    } else {
                        names.first().cloned().unwrap_or_else(|| "af_heart".into())
                    }
                }
            };
            Ok((Backend::Kokoro { model_dir, voice: chosen.clone(), all_voices: names, speed: 1.0 }, chosen))
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

/// Fetch a URL into Odoru's document store, cache-aware (scans existing
/// documents for a matching `source_url` unless `no_cache`), returning the
/// parsed article and the resulting document id.
fn fetch_url_to_document(input: &str, no_cache: bool) -> anyhow::Result<(ParsedArticle, String)> {
    if !no_cache {
        let docs = documents::list_all()?;
        if let Some(hit) = docs.into_iter().find(|d| d.source_url.as_deref() == Some(input)) {
            if hit.status == documents::FetchStatus::Ready {
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
                    return Ok((article, hit.id));
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

    Ok((article, id))
}

/// Load article content from a local file or URL.
///
/// Local files: `.txt` (read directly), `.md` (read directly, plain text
/// derived via `tts::markdown::to_plain_text`), or `.html` (extract via
/// trafilatura). Any other extension is an error. If the input is not an
/// existing file path, it is treated as a URL.
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
            "md" => {
                let content = std::fs::read_to_string(path)?;
                let plain_text = tts::markdown::to_plain_text(&content);
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
                    content,
                    plain_text,
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
                "Unsupported file extension '.{other}'. Only .txt, .md, and .html are supported."
            ),
        }
    } else if input.starts_with("http://") || input.starts_with("https://") {
        let (article, _id) = fetch_url_to_document(input, no_cache)?;
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
    match cli.command {
        Command::Fetch(args) => run_fetch(args).await,
        Command::Spa(args) => run_spa(args),
        Command::Import(args) => run_import(args).await,
        Command::Export(args) => run_export(args),
        Command::Edit(args) => run_edit(args),
    }
}

fn run_export(args: ExportArgs) -> anyhow::Result<()> {
    let doc = documents::lookup_by_id(&args.doc_id)
        .with_context(|| format!("looking up document {}", args.doc_id))?
        .ok_or_else(|| anyhow::anyhow!("no document found with id {}", args.doc_id))?;

    let text = match args.format {
        Format::Text => &doc.plain_text,
        Format::Markdown => &doc.content,
    };

    std::fs::write(&args.path, text)
        .with_context(|| format!("writing {}", args.path.display()))?;

    println!("wrote {}", args.path.display());
    Ok(())
}

fn run_edit(args: EditArgs) -> anyhow::Result<()> {
    let file_text = std::fs::read_to_string(&args.path)
        .with_context(|| format!("reading {}", args.path.display()))?;

    let (content, plain_text) = match args.format {
        Format::Text => (file_text.clone(), file_text),
        Format::Markdown => {
            let plain_text = tts::markdown::to_plain_text(&file_text);
            (file_text, plain_text)
        }
    };

    documents::update_content(&args.doc_id, &content, &plain_text)
        .with_context(|| format!("updating document {}", args.doc_id))?;

    println!("updated doc id: {}", args.doc_id);
    Ok(())
}

async fn run_import(args: ImportArgs) -> anyhow::Result<()> {
    match args.command {
        ImportCommand::Fetch(fetch_args) => run_import_fetch(fetch_args).await,
        ImportCommand::Vibe(vibe_args) => {
            tokio::task::spawn_blocking(move || import::run_vibe(&vibe_args.basedir)).await?
        }
    }
}

async fn run_import_fetch(args: ImportFetchArgs) -> anyhow::Result<()> {
    if !(args.url.starts_with("http://") || args.url.starts_with("https://")) {
        anyhow::bail!(
            "'{}' does not look like a URL (expected http:// or https://)",
            args.url
        );
    }

    let url = args.url.clone();
    let no_cache = args.no_cache;
    let (_article, id) = tokio::task::spawn_blocking(move || fetch_url_to_document(&url, no_cache))
        .await??;

    if args.json {
        println!("{}", serde_json::json!({ "id": id }));
    } else {
        println!("doc id: {id}");
    }
    Ok(())
}

async fn run_fetch(args: FetchArgs) -> anyhow::Result<()> {
    let sp = spinner(&format!("Loading {}...", args.input));
    let input = args.input.clone();
    let input_display = args.input.clone();
    let no_cache = args.no_cache;
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

    if args.frontmatter {
        print!("{}", render_frontmatter(&article));
    }
    let content = match args.format {
        Format::Text => double_space_paragraphs(&article.plain_text),
        Format::Markdown => article.content.clone(),
    };
    print!("{}", content);

    if args.audio {
        let stem = wav_path_override
            .unwrap_or_else(|| mp3_filename(&article));
        let mp3_path = resolve_mp3_path(args.output.as_deref(), &stem)?;
        let (backend, voice_name) = build_backend(args.backend, args.voice.as_deref())?;
        let engine = TtsEngine::builder().backend(backend).build()?;
        let total = tts::splitter::split(&article.plain_text).len();
        let pb = audio_progress(total);
        audio::synthesize_to_mp3(&article.plain_text, mp3_path.to_str().unwrap(), &engine, &voice_name, &pb).await?;
        pb.finish_with_message(format!("✔ Audio saved to {}", mp3_path.display()));
    }

    Ok(())
}

fn run_spa(args: SpaArgs) -> anyhow::Result<()> {
    use util::export::{ExportTranscriptEntry, ManifestEntry};
    use util::slug::export_slug;

    let out = std::path::PathBuf::from(&args.output_dir);
    std::fs::create_dir_all(&out)
        .with_context(|| format!("failed to create output directory {}", out.display()))?;

    // ── Collect published documents ──────────────────────────────────────
    let all_docs = documents::list_all()?;
    let published: Vec<_> = all_docs.into_iter().filter(|d| d.publish).collect();

    if published.is_empty() {
        eprintln!("Warning: no documents with publish: true found — exporting empty site");
    }

    let mut manifest: Vec<ManifestEntry> = Vec::new();
    let mut transcripts: std::collections::HashMap<String, Vec<ExportTranscriptEntry>> =
        std::collections::HashMap::new();
    let mut doc_contents: std::collections::HashMap<String, serde_json::Value> =
        std::collections::HashMap::new();

    for doc in &published {
        // Need content — re-fetch with full body.
        let full = match documents::lookup_by_id(&doc.id)? {
            Some(d) => d,
            None => { eprintln!("Warning: could not re-read document {}", doc.id); continue; }
        };

        let meta = full.export_meta();
        let slug = export_slug(meta.title.as_deref(), meta.date.as_deref());

        // ── Attempt audio export ─────────────────────────────────────────
        let has_audio = match &meta.voice_id {
            None => {
                eprintln!(
                    "Warning: '{}' has no published voice — exporting text only",
                    meta.title.as_deref().unwrap_or(&slug)
                );
                false
            }
            Some(voice_id) => {
                match resolve_voice(voice_id) {
                    Err(e) => {
                        eprintln!(
                            "Warning: '{}' — could not load voice '{voice_id}': {e} — exporting text only",
                            meta.title.as_deref().unwrap_or(&slug)
                        );
                        false
                    }
                    Ok(voice) => {
                        let audio_entries = tts::export::export_audio(&full.plain_text, &voice);
                        let miss = audio_entries.iter().find(|e| e.mp3.is_none());
                        if let Some(m) = miss {
                            eprintln!(
                                "Warning: '{}' — audio cache miss for sentence {}: {:?} — exporting text only",
                                meta.title.as_deref().unwrap_or(&slug),
                                m.index,
                                &m.text[..m.text.len().min(60)]
                            );
                            false
                        } else {
                            // Write MP3 files
                            let audio_dir = out.join("documents").join(&slug).join("audio");
                            std::fs::create_dir_all(&audio_dir)
                                .with_context(|| format!("failed to create {}", audio_dir.display()))?;
                            for entry in &audio_entries {
                                let filename = format!("{:04}.mp3", entry.index);
                                std::fs::write(audio_dir.join(&filename), entry.mp3.as_ref().unwrap())
                                    .with_context(|| format!("failed to write {filename}"))?;
                            }
                            // Populate transcript timing
                            let mut cursor = 0.0_f64;
                            let timed: Vec<ExportTranscriptEntry> = audio_entries.iter().map(|e| {
                                let start = cursor;
                                let end = cursor + e.duration;
                                cursor = end;
                                ExportTranscriptEntry {
                                    index: e.index,
                                    text: e.text.clone(),
                                    start,
                                    end,
                                    paragraph_end: e.paragraph_end,
                                }
                            }).collect();
                            transcripts.insert(slug.clone(), timed);
                            true
                        }
                    }
                }
            }
        };

        // Fall back to timing-less transcript if audio wasn't produced above
        if !has_audio && !transcripts.contains_key(&slug) {
            let entries: Vec<ExportTranscriptEntry> = tts::splitter::split(&full.plain_text)
                .into_iter()
                .enumerate()
                .map(|(i, s)| ExportTranscriptEntry {
                    index: i, text: s.text, start: 0.0, end: 0.0,
                    paragraph_end: s.paragraph_end,
                })
                .collect();
            transcripts.insert(slug.clone(), entries);
        }

        manifest.push(ManifestEntry {
            slug: slug.clone(),
            title: meta.title.unwrap_or_else(|| slug.clone()),
            authors: meta.authors,
            date: meta.date,
            description: full.description.clone(),
            source_url: meta.source_url.clone(),
            has_audio,
        });
        doc_contents.insert(slug, serde_json::json!({
            "content": full.content,
            "plain_text": full.plain_text,
        }));
    }

    // ── Serialize window.__ODORU__ payload ───────────────────────────────
    let odoru_json = serde_json::to_string(&serde_json::json!({
        "manifest": manifest,
        "transcripts": transcripts,
        "documents": doc_contents,
    })).context("failed to serialize __ODORU__ payload")?;

    // ── Build self-contained index.html ──────────────────────────────────
    let dist = find_frontend_dist()?;
    let reader_html_path = dist.join("reader-export.html");
    if !reader_html_path.exists() {
        anyhow::bail!(
            "reader-export.html not found in {}.\nRun: cd app/frontend && npm run build",
            dist.display()
        );
    }

    let mut html = inline_assets(&reader_html_path, &dist)
        .context("failed to inline assets into reader-export.html")?;

    // Inject window.__ODORU__ before </head>.
    let injection = format!("<script>window.__ODORU__ = {};</script>\n</head>", odoru_json);
    html = html.replacen("</head>", &injection, 1);

    std::fs::write(out.join("index.html"), &html)
        .context("failed to write index.html")?;

    let favicon_src = dist.join("favicon.ico");
    if favicon_src.exists() {
        std::fs::copy(&favicon_src, out.join("favicon.ico"))
            .context("failed to copy favicon.ico")?;
    }

    println!("✔ Exported {} document(s) to {}", manifest.len(), out.display());
    Ok(())
}

/// Read `html_path`, replace every `<link rel="stylesheet" href="...">` and
/// `<script ... src="...">` that point into `dist_dir` with inline
/// `<style>` / `<script>` blocks.  Also strips `crossorigin` attributes.
fn inline_assets(html_path: &std::path::Path, dist_dir: &std::path::Path) -> anyhow::Result<String> {
    let html = std::fs::read_to_string(html_path)
        .with_context(|| format!("failed to read {}", html_path.display()))?;

    let mut out = String::with_capacity(html.len() * 2);
    let mut rest = html.as_str();

    while !rest.is_empty() {
        // Try to find the next tag we care about.
        if let Some(tag_start) = rest.find('<') {
            out.push_str(&rest[..tag_start]);
            rest = &rest[tag_start..];

            // <link rel="stylesheet" ... href="./assets/foo.css" ...>
            if rest.starts_with("<link") {
                if let Some(tag_end) = rest.find('>') {
                    let tag = &rest[..=tag_end];
                    if tag.contains("stylesheet") {
                        if let Some(href) = extract_attr(tag, "href") {
                            let asset_path = dist_dir.join(href.trim_start_matches("./"));
                            if asset_path.exists() {
                                let css = std::fs::read_to_string(&asset_path)
                                    .with_context(|| format!("failed to read {}", asset_path.display()))?;
                                out.push_str("<style>\n");
                                out.push_str(&css);
                                out.push_str("\n</style>");
                                rest = &rest[tag_end + 1..];
                                continue;
                            }
                        }
                    }
                    // modulepreload links — drop them, they're redundant once inlined
                    if tag.contains("modulepreload") {
                        rest = &rest[tag_end + 1..];
                        continue;
                    }
                    out.push_str(tag);
                    rest = &rest[tag_end + 1..];
                    continue;
                }
            }

            // <script ... src="./assets/foo.js" ...></script>
            if rest.starts_with("<script") {
                if let Some(tag_end) = rest.find('>') {
                    let open_tag = &rest[..=tag_end];
                    // Find closing </script>
                    let after_open = &rest[tag_end + 1..];
                    let close = after_open.find("</script>").unwrap_or(after_open.len());
                    let full_end = tag_end + 1 + close + "</script>".len();

                    if let Some(src) = extract_attr(open_tag, "src") {
                        let asset_path = dist_dir.join(src.trim_start_matches("./"));
                        if asset_path.exists() {
                            let js = std::fs::read_to_string(&asset_path)
                                .with_context(|| format!("failed to read {}", asset_path.display()))?;
                            if js.contains("import{") || js.contains("import {") {
                                anyhow::bail!(
                                    "{} still contains an ES import after the build — \
                                     reader-export.html must be built as a single self-contained \
                                     bundle (see vite.reader-export.config.ts, codeSplitting: false)",
                                    asset_path.display()
                                );
                            }
                            out.push_str("<script type=\"module\">\n");
                            out.push_str(&js);
                            out.push_str("\n</script>");
                            rest = &rest[full_end..];
                            continue;
                        }
                    }
                    // Script tag with no src (or unresolved) — pass through as-is
                    out.push_str(&rest[..full_end]);
                    rest = &rest[full_end..];
                    continue;
                }
            }

            // Any other tag — pass one character and continue
            out.push('<');
            rest = &rest[1..];
        } else {
            out.push_str(rest);
            break;
        }
    }

    Ok(out)
}

/// Extract the value of a named attribute from an HTML tag string.
fn extract_attr<'a>(tag: &'a str, attr: &str) -> Option<&'a str> {
    let needle_dq = format!("{attr}=\"");
    let needle_sq = format!("{attr}='");
    if let Some(pos) = tag.find(&needle_dq) {
        let start = pos + needle_dq.len();
        let end = tag[start..].find('"')? + start;
        return Some(&tag[start..end]);
    }
    if let Some(pos) = tag.find(&needle_sq) {
        let start = pos + needle_sq.len();
        let end = tag[start..].find('\'')? + start;
        return Some(&tag[start..end]);
    }
    None
}

/// Resolve a voice ID string (e.g. `"f5:sarah"`, `"kokoro:af_heart"`) to a
/// `tts::Voice` suitable for audio cache key computation.
fn resolve_voice(voice_id: &str) -> anyhow::Result<tts::Voice> {
    if let Some(name) = voice_id.strip_prefix("kokoro:") {
        return Ok(tts::Voice::kokoro(name));
    }
    if let Some(name) = voice_id.strip_prefix("f5:") {
        let voices_dir = util::voice::voices_dir()
            .map_err(|e| anyhow::anyhow!("cannot find voices directory: {e}"))?;
        let def = util::voice::VoiceDef::load(&voices_dir.join(name))
            .map_err(|e| anyhow::anyhow!("failed to load voice '{name}': {e}"))?;
        return Ok(tts::Voice::F5Tts {
            name: def.name,
            voice_ref: def.voice_ref,
            ref_text: def.ref_text,
            speed: def.speed,
            cfg_strength: def.cfg_strength,
        });
    }
    anyhow::bail!("unrecognised voice ID format: {voice_id:?} (expected 'f5:NAME' or 'kokoro:NAME')")
}

/// Search for the built frontend dist directory.
fn find_frontend_dist() -> anyhow::Result<std::path::PathBuf> {
    let candidates = [
        "app/frontend/dist",
        "frontend/dist",
        "../app/frontend/dist",
        "../frontend/dist",
    ];
    for candidate in candidates {
        let p = std::path::PathBuf::from(candidate);
        if p.is_dir() {
            return Ok(p);
        }
    }
    // Also honour an explicit env var override.
    if let Ok(dir) = std::env::var("ODORU_FRONTEND_DIST") {
        let p = std::path::PathBuf::from(dir);
        if p.is_dir() {
            return Ok(p);
        }
    }
    anyhow::bail!(
        "Could not find app/frontend/dist. Run: cd app/frontend && npm run build\n\
         Or set ODORU_FRONTEND_DIST to its path."
    )
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
    format!(
        "{}.mp3",
        util::slug::export_slug(article.title.as_deref(), article.date.as_deref())
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize tests that mutate process-global env vars.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

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
        assert_eq!(voice_name, "af_heart");
        match backend {
            tts::Backend::Kokoro { model_dir, voice, .. } => {
                assert_eq!(model_dir, std::path::PathBuf::from("/tmp/kokoro-test"));
                assert_eq!(voice, "af_heart");
            }
            _ => panic!("expected Kokoro backend"),
        }
    }

    #[test]
    fn build_backend_kokoro_unknown_voice_errors() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("KOKORO_MODEL_DIR", "/tmp/kokoro-test");
        let result = build_backend(AudioBackend::Kokoro, Some("nobody"));
        std::env::remove_var("KOKORO_MODEL_DIR");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("nobody"));
        assert!(msg.contains("Available"));
    }

    #[test]
    fn build_backend_kokoro_selects_named_voice() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("KOKORO_MODEL_DIR", "/tmp/kokoro-test");
        let (backend, voice_name) = build_backend(AudioBackend::Kokoro, Some("af_heart")).unwrap();
        std::env::remove_var("KOKORO_MODEL_DIR");
        assert_eq!(voice_name, "af_heart");
        match backend {
            tts::Backend::Kokoro { voice, .. } => assert_eq!(voice, "af_heart"),
            _ => panic!("expected Kokoro backend"),
        }
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

    // Writes a minimal fixture voice dir so F5 tests don't depend on the
    // repo's real voices/ directory (whose contents change over time).
    fn write_f5_voice_fixture(dir: &std::path::Path, name: &str) {
        let voice_dir = dir.join(name);
        std::fs::create_dir(&voice_dir).unwrap();
        std::fs::write(voice_dir.join("voice.md"), "---\ntranscript: \"Hi.\"\n---\n").unwrap();
        std::fs::write(voice_dir.join("ref.wav"), b"").unwrap();
    }

    #[test]
    fn build_backend_f5_loads_first_voice_alphabetically() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        write_f5_voice_fixture(dir.path(), "sarah");
        write_f5_voice_fixture(dir.path(), "alice");
        std::env::set_var("VOICES_DIR", dir.path());
        let result = build_backend(AudioBackend::F5, None);
        std::env::remove_var("VOICES_DIR");
        let (backend, voice_name) = result.unwrap();
        assert_eq!(voice_name, "alice"); // first alphabetically
        let _ = backend; // just check it doesn't error
    }

    #[test]
    fn build_backend_f5_selects_named_voice() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        write_f5_voice_fixture(dir.path(), "sarah");
        write_f5_voice_fixture(dir.path(), "alice");
        std::env::set_var("VOICES_DIR", dir.path());
        let result = build_backend(AudioBackend::F5, Some("sarah"));
        std::env::remove_var("VOICES_DIR");
        let (backend, voice_name) = result.unwrap();
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
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        write_f5_voice_fixture(dir.path(), "sarah");
        std::env::set_var("VOICES_DIR", dir.path());
        let result = build_backend(AudioBackend::F5, Some("nobody"));
        std::env::remove_var("VOICES_DIR");
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
