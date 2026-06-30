mod config;
mod runpod;
mod segment;
mod synth;
mod voice;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use synth::SynthInput;
use tracing::{info, warn};

#[derive(Parser)]
#[command(allow_negative_numbers = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List GPU types with pricing
    GpuTypes,
    /// List RunPod templates
    ListTemplates,
    /// List RunPod pods
    ListPods,
    /// Get a single pod's details
    PodStatus { pod_id: String },
    /// Start a pod
    StartPod { pod_id: String },
    /// Stop a pod
    StopPod { pod_id: String },
    /// Terminate (delete) a pod
    TerminatePod { pod_id: String },
    /// Print the SSH connection command for a running pod
    Ssh { pod_id: String },
    /// Download a file from a running pod via scp
    Download {
        pod_id: String,
        remote_path: String,
        /// Local destination (defaults to vibe/<basename of remote_path>)
        local_path: Option<String>,
    },
    /// Create a new pod from a template (uses the only template if
    /// none specified). Auto-selects cheapest GPU with >=24GB VRAM
    /// unless --gpu-type-id is given.
    NewPod {
        compute_type: runpod::ComputeType,
        template_id: Option<String>,
        /// Network volume to attach (omit to avoid region lock)
        #[arg(long)]
        network_volume_id: Option<String>,
        /// Pod name
        #[arg(short, long, default_value = "vibevoice")]
        name: String,
        /// GPU type id (see `gpu-types`), e.g. "NVIDIA A40". If omitted,
        /// auto-selects cheapest GPU with >=24GB VRAM.
        #[arg(long)]
        gpu_type_id: Option<String>,
    },
    /// Poll a pod every 10 minutes and send a macOS notification while it
    /// is still running. Exits when the pod is gone. Auto-launched by
    /// listen-test on completion.
    WatchPod {
        pod_id: String,
        /// Poll interval in seconds (default 600)
        #[arg(long, default_value_t = 600)]
        interval_secs: u64,
    },
    /// Normalize text (calls util::normalizer::normalize)
    Normalize { text: String },
    /// Split a document into TTS segments and write them as numbered files.
    /// Reads <basedir>/<name>.txt, writes <basedir>/<name>_seg01.txt etc.
    /// with Speaker 1: prefix per paragraph.
    Segment {
        /// Stem of <basedir>/<name>.txt (no extension)
        name: String,
        /// Directory for segment output (default: vibe/data). Use this to
        /// keep separate synthesis runs for the same document apart — if
        /// more than one exists there's no marker for which is "current",
        /// so the caller always names the one they mean.
        #[arg(long)]
        basedir: Option<String>,
    },
    /// Print a per-segment status table for `<basedir>/<name>.segments.json`:
    /// which segments are missing audio (not yet synthesized) and the QA
    /// verdict for those that are done. Lets you resume a run without
    /// re-reading every log line.
    Summary {
        /// Stem of <basedir>/<name>.txt (no extension)
        name: String,
        /// Directory holding the sidecar and segment output (default:
        /// vibe/data). Use the same --basedir you used for `segment`.
        #[arg(long)]
        basedir: Option<String>,
    },
    /// Regenerate `<basedir>/<name>.segments.json` from whatever
    /// <name>_segNN.txt files currently exist on disk, instead of from
    /// `segment`'s original split. Use after hand-editing segment files
    /// (e.g. testing a segmenter fix) to get a sidecar that matches
    /// reality without re-running `segment` and losing recorded synthesis
    /// output for untouched segments.
    SegmentsFromFiles {
        /// Stem of <basedir>/<name>.txt (no extension)
        name: String,
        /// Directory holding the segment files and sidecar (default:
        /// vibe/data). Use the same --basedir you used for `segment`.
        #[arg(long)]
        basedir: Option<String>,
        /// Voice ID to record if there's no existing sidecar to read one
        /// from. Ignored if a sidecar with a voice_id already exists.
        #[arg(long)]
        voice_id: Option<String>,
    },
    /// Upload a reference voice wav to a running pod's vibe-service,
    /// without baking it into the (public) Docker image. Persists only
    /// for the pod's lifetime — re-upload after creating a new pod.
    UploadVoice {
        /// RunPod pod id (omit if --url is given)
        #[arg(long)]
        pod_id: Option<String>,
        /// Voice name (e.g. "Andy") — pass as --speaker to synthesize afterward.
        #[arg(long)]
        name: String,
        /// Voice descriptor matching VibeVoice's filename convention, e.g. "man" or "woman".
        #[arg(long)]
        gender: String,
        /// Local path to the reference wav file.
        #[arg(long)]
        wav_path: String,
        #[arg(long, default_value_t = 3000)]
        port: u16,
        /// Base URL of a vibe-service instance (e.g. a Cloud Run service
        /// URL), used instead of deriving a RunPod proxy URL from pod_id.
        #[arg(long)]
        url: Option<String>,
    },
    /// Run ffmpeg silencedetect on a local wav file
    Silencedetect {
        wav_path: String,
        #[arg(long, default_value_t = -35.0, allow_hyphen_values = true)]
        noise_db: f64,
        #[arg(long, default_value_t = 0.5)]
        duration: f64,
    },
    /// Run VibeVoice inference via the vibe-service HTTP API running on
    /// the pod. Normalizes and synthesizes a pre-split segment or a whole
    /// document (--doc splits it first). Downloads the wav and appends
    /// runs.jsonl.
    Synthesize {
        /// What to synthesize: a pre-split segment file or a whole document.
        #[command(subcommand)]
        input: SynthInput,
        /// RunPod pod id (omit if --url is given)
        pod_id: Option<String>,
        /// Base URL of a vibe-service instance (e.g. a Cloud Run service
        /// URL), used instead of deriving a RunPod proxy URL from pod_id.
        /// Skips RunPod-specific direct-IP lookup when set.
        #[arg(long)]
        url: Option<String>,
        #[arg(long, default_value = "Sarah")]
        speaker: String,
        /// Named voice from vibe/voices/<name>/voice.md — auto-uploads
        /// ref.wav before synthesizing and supplies cfg_scale/seed/speed/temp
        /// defaults from voice.md (CLI flags still take precedence over
        /// those defaults). Mutually exclusive with --speaker.
        #[arg(long, conflicts_with = "speaker")]
        voice: Option<String>,
        /// Classifier-free guidance scale. Defaults to the voice's value (if
        /// --voice is given) or 1.3.
        #[arg(long)]
        cfg_scale: Option<f64>,
        /// Sampling temperature. When set, enables sampling (non-deterministic);
        /// omit for greedy/deterministic generation.
        #[arg(long)]
        temp: Option<f64>,
        /// Voice speed factor applied to the reference audio. <1 slows the
        /// cloned voice, >1 speeds it up; omit (or 1.0) to leave unchanged.
        #[arg(long)]
        speed: Option<f64>,
        /// Random seed. Defaults to the voice's value (if --voice is given)
        /// or 71463.
        #[arg(long)]
        seed: Option<u64>,
        /// GPU price per hour (from new-pod output), stored in run log
        #[arg(long)]
        gpu_price: Option<f64>,
        /// Override the service port (default: 3000)
        #[arg(long, default_value_t = 3000)]
        port: u16,
        /// Directory holding the segment file and where output (wav,
        /// normalized text, transcript, report) is written (default:
        /// vibe/data). Use this to keep separate synthesis runs for the
        /// same document apart — if more than one exists there's no
        /// marker for which is "current", so the caller always names
        /// the one they mean.
        #[arg(long)]
        basedir: Option<String>,
    },
}


/// Poll `<base_url>/health` until the service reports ready, then return its
/// reported GPU name and VRAM (in MiB), if any.
pub(crate) async fn wait_for_health(http: &reqwest::Client, base_url: &str) -> Result<(String, Option<u64>)> {
    loop {
        match http.get(format!("{base_url}/health")).send().await {
            Ok(r) if r.status().is_success() => {
                let body: serde_json::Value = r.json().await.unwrap_or_default();
                if body.get("status").and_then(|v| v.as_str()) == Some("ready") {
                    let gpu_info = body.get("gpu").and_then(|v| v.as_str()).unwrap_or("unknown");
                    info!("service ready — GPU: {gpu_info}");
                    let mut parts = gpu_info.splitn(2, ',');
                    let gpu_name = parts.next().unwrap_or("unknown").trim().to_string();
                    let gpu_vram_mb = parts.next()
                        .and_then(|s| s.trim().trim_end_matches(" MiB").parse::<u64>().ok());
                    return Ok((gpu_name, gpu_vram_mb));
                }
                if let Some(msg) = body.get("message").and_then(|v| v.as_str()) {
                    anyhow::bail!("service reported error: {msg}");
                }
            }
            Ok(r) => info!("health: HTTP {} — retrying", r.status()),
            Err(e) => info!("health: {e} — retrying"),
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

/// Run a command, streaming its output, and bail if it fails.
fn run(argv: &[String]) -> Result<()> {
    info!("running: {}", argv.join(" "));
    let status = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .status()?;
    if !status.success() {
        anyhow::bail!("command failed: {status}");
    }
    Ok(())
}

/// Run a command and return its captured stdout (trimmed), bailing on
/// non-zero exit.
#[allow(dead_code)] // kept for a future SSH-fallback path; no current caller since listen-test-ssh was removed
fn run_output(argv: &[String]) -> Result<String> {
    let output = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .output()?;
    if !output.status.success() {
        anyhow::bail!(
            "command failed: {}\nstderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}


#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    let api_key = config::runpod_api_key()?;
    let client = runpod::Client::new(api_key);

    match cli.command {
        Command::GpuTypes => {
            let gpu_types = client.list_gpu_types().await?;
            println!("{}", serde_json::to_string_pretty(&gpu_types)?);
        }
        Command::ListTemplates => {
            let templates = client.list_templates().await?;
            println!("{}", serde_json::to_string_pretty(&templates)?);
        }
        Command::ListPods => {
            let pods = client.list_pods().await?;
            println!("{}", serde_json::to_string_pretty(&pods)?);
        }
        Command::PodStatus { pod_id } => {
            let pod = client.get_pod(&pod_id).await?;
            println!("{}", serde_json::to_string_pretty(&pod)?);
        }
        Command::StartPod { pod_id } => {
            let pod = client.start_pod(&pod_id).await?;
            println!("{}", serde_json::to_string_pretty(&pod)?);
        }
        Command::StopPod { pod_id } => {
            let pod = client.stop_pod(&pod_id).await?;
            println!("{}", serde_json::to_string_pretty(&pod)?);
        }
        Command::TerminatePod { pod_id } => {
            client.terminate_pod(&pod_id).await?;
            info!("Terminated pod {pod_id}");
        }
        Command::UploadVoice { pod_id, name, gender, wav_path, port, url } => {
            let bytes = std::fs::read(&wav_path).with_context(|| format!("reading {wav_path}"))?;
            info!("uploading {wav_path} ({} bytes) as voice {name}/{gender}", bytes.len());

            let base_url = match url {
                Some(url) => url,
                None => {
                    let pod_id = pod_id.context("pod_id or --url is required")?;
                    format!("https://{pod_id}-{port}.proxy.runpod.net")
                }
            };
            let secret = std::env::var("VIBE_SERVICE_SECRET").ok();
            let http = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()?;

            info!("waiting for vibe-service at {base_url}/health ...");
            wait_for_health(&http, &base_url).await?;
            voice::upload_wav(&http, &base_url, &name, &gender, bytes, &secret).await?;
            info!("uploaded voice {name}/{gender} — pass --speaker {name} to synthesize");
        }
        Command::Ssh { pod_id } => {
            let pod = client.get_pod(&pod_id).await?;
            let cmd = runpod::ssh_command(&pod_id, &pod)?;
            println!("{cmd}");
        }
        Command::Download {
            pod_id,
            remote_path,
            local_path,
        } => {
            let pod = client.get_pod(&pod_id).await?;
            let local_path = local_path.unwrap_or_else(|| {
                let basename = remote_path.rsplit('/').next().unwrap_or(&remote_path);
                format!(
                    "{}/{basename}",
                    concat!(env!("CARGO_MANIFEST_DIR"))
                )
            });
            let argv = runpod::scp_download_command(&pod_id, &pod, &remote_path, &local_path)?;
            run(&argv)?;
            info!("downloaded to {local_path}");
        }
        Command::NewPod { compute_type, template_id, network_volume_id, name, gpu_type_id } => {
            let template_id = client.resolve_template(template_id).await?;
            info!("using template: {template_id}");
            let network_volume_id = network_volume_id;

            // Build candidate GPU list sorted by price ascending (>=24GB VRAM).
            // If a specific gpu_type_id was given, use only that one.
            let candidates: Vec<(f64, String, String, f64)> = if let Some(id) = gpu_type_id {
                vec![(0.0, id.clone(), id, 0.0)]
            } else if matches!(compute_type, runpod::ComputeType::Gpu) {
                let gpu_types = client.list_gpu_types().await?;
                let arr = gpu_types.as_array().context("gpu-types not an array")?;
                let mut list: Vec<_> = arr.iter()
                    .filter_map(|g| {
                        let vram = g["memoryInGb"].as_f64()?;
                        let price = g["lowestPrice"]["uninterruptablePrice"].as_f64()?;
                        let id = g["id"].as_str()?;
                        let label = g["displayName"].as_str().unwrap_or(id);
                        if vram >= 24.0 { Some((price, id.to_string(), label.to_string(), vram)) }
                        else { None }
                    })
                    .collect();
                list.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
                list
            } else {
                vec![]
            };

            if candidates.is_empty() && matches!(compute_type, runpod::ComputeType::Gpu) {
                warn!("no GPU with >=24GB VRAM found; letting RunPod choose");
            }

            // Try candidates in price order, falling back to the next one on
            // any creation error — covers both "no instances available" and
            // GPUs RunPod's GraphQL list exposes but its REST create-pod
            // endpoint doesn't yet support (e.g. newly added hardware).
            let mut chosen_price: Option<f64> = None;
            let no_candidates = candidates.is_empty();
            let mut iter = candidates.into_iter().peekable();
            let pod = loop {
                let gpu_id = iter.next().map(|(price, id, label, vram)| {
                    info!("trying GPU: {} ({}GB VRAM, ${:.2}/hr)", label, vram, price);
                    chosen_price = Some(price);
                    id
                });
                match client.create_pod(&template_id, compute_type, network_volume_id.as_deref(), &name, gpu_id.as_deref()).await {
                    Ok(p) => break p,
                    Err(e) => {
                        if no_candidates || iter.peek().is_none() {
                            anyhow::bail!("no available GPU found after trying all candidates: {e}");
                        }
                        warn!("GPU unavailable ({e}), trying next GPU...");
                    }
                }
            };
            let pod_id = pod.get("id").and_then(|v| v.as_str()).context("created pod missing id")?;
            info!("created pod: {pod_id}");
            if let Some(price) = chosen_price {
                info!("estimated cost: ${price:.2}/hr — pass --gpu-price {price:.2} to listen-test for run log");
            }
        }
        Command::Normalize { text } => {
            println!("{}", util::normalizer::normalize(&text));
        }
        Command::Segment { name, basedir } => {
            segment::run(&name, basedir.as_deref())?;
        }
        Command::Summary { name, basedir } => {
            segment::summary(&name, basedir.as_deref())?;
        }
        Command::SegmentsFromFiles { name, basedir, voice_id } => {
            segment::segments_from_files(&name, basedir.as_deref(), voice_id.as_deref())?;
        }
        Command::Synthesize {
            input,
            pod_id,
            url,
            speaker,
            voice,
            cfg_scale,
            temp,
            speed,
            seed,
            gpu_price,
            port,
            basedir,
        } => {
            synth::run(
                &client, input, pod_id, url, speaker, voice, cfg_scale, temp, speed, seed,
                gpu_price, port, basedir,
            )
            .await?;
        }
        Command::WatchPod { pod_id, interval_secs } => {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(interval_secs)).await;
                match client.get_pod(&pod_id).await {
                    Ok(pod) => {
                        let desired = pod["desiredStatus"].as_str().unwrap_or("");
                        let runtime = &pod["runtime"];
                        let is_running = desired == "RUNNING" && !runtime.is_null();
                        if !is_running {
                            info!("watch-pod: {pod_id} is no longer running, exiting");
                            break;
                        }
                        let msg = format!("Pod {} is still running — remember to terminate it!", pod_id);
                        let _ = std::process::Command::new("osascript")
                            .args(["-e", &format!(
                                "display notification \"{}\" with title \"RunPod\" sound name \"Basso\"",
                                msg
                            )])
                            .status();
                        info!("watch-pod: notified (pod {pod_id} still running)");
                    }
                    Err(e) => {
                        warn!("watch-pod: failed to get pod status: {e}");
                        break;
                    }
                }
            }
        }
        Command::Silencedetect {
            wav_path,
            noise_db,
            duration,
        } => {
            let filter = format!("silencedetect=noise={noise_db}dB:d={duration}");
            let output = std::process::Command::new("ffmpeg")
                .args(["-i", &wav_path, "-af", &filter, "-f", "null", "-"])
                .output()?;
            let stderr = String::from_utf8_lossy(&output.stderr);
            for line in stderr.lines() {
                if line.to_lowercase().contains("silence") {
                    println!("{line}");
                }
            }
        }
    }

    Ok(())
}
