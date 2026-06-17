mod config;
mod runpod;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
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
    /// none specified). Auto-selects cheapest GPU with >=10GB VRAM
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
        /// auto-selects cheapest GPU with >=10GB VRAM.
        #[arg(long)]
        gpu_type_id: Option<String>,
    },
    /// Normalize text (calls tts::f5::normalizer::normalize)
    Normalize { text: String },
    /// Run ffmpeg silencedetect on a local wav file
    Silencedetect {
        wav_path: String,
        #[arg(long, default_value_t = -35.0, allow_hyphen_values = true)]
        noise_db: f64,
        #[arg(long, default_value_t = 0.5)]
        duration: f64,
    },
    /// Run the full listen-test loop: normalize data/<name>.txt, upload,
    /// run VibeVoice inference on the pod, wait for completion, and
    /// download the resulting wav to data/<name>_generated.wav.
    ListenTest {
        /// Stem of vibe/data/<name>.txt (no extension)
        name: String,
        pod_id: String,
        #[arg(long, default_value = "Sarah")]
        speaker: String,
        #[arg(long, default_value_t = 1.3)]
        cfg_scale: f64,
        /// Seconds between checks of the inference log
        #[arg(long, default_value_t = 30)]
        poll_secs: u64,
        /// Give up waiting for inference after this many minutes
        #[arg(long, default_value_t = 60)]
        timeout_mins: u64,
        /// Random seed for reproducibility (omit for random)
        #[arg(long)]
        seed: Option<u64>,
        /// GPU price per hour (from new-pod output), stored in run log
        #[arg(long)]
        gpu_price: Option<f64>,
    },
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

/// Append a JSON line to data/runs.jsonl for later cost/perf analysis.
fn append_run_log(entry: serde_json::Value) -> Result<()> {
    let data_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/data");
    let path = format!("{data_dir}/runs.jsonl");
    let line = serde_json::to_string(&entry)? + "\n";
    use std::io::Write;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?
        .write_all(line.as_bytes())?;
    Ok(())
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

            // Build candidate GPU list sorted by price ascending (>=10GB VRAM).
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
                        if vram >= 10.0 { Some((price, id.to_string(), label.to_string(), vram)) }
                        else { None }
                    })
                    .collect();
                list.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
                list
            } else {
                vec![]
            };

            if candidates.is_empty() && matches!(compute_type, runpod::ComputeType::Gpu) {
                warn!("no GPU with >=10GB VRAM found; letting RunPod choose");
            }

            // Try candidates in price order, falling back on "not available" errors.
            let mut pod = None;
            let mut chosen_price: Option<f64> = None;
            let no_candidates = candidates.is_empty();
            let mut iter = candidates.into_iter().peekable();
            loop {
                let gpu_id = iter.next().map(|(price, id, label, vram)| {
                    info!("trying GPU: {} ({}GB VRAM, ${:.2}/hr)", label, vram, price);
                    chosen_price = Some(price);
                    id
                });
                match client.create_pod(&template_id, compute_type, network_volume_id.as_deref(), &name, gpu_id.as_deref()).await {
                    Ok(p) => { pod = Some(p); break; }
                    Err(e) => {
                        let msg = e.to_string();
                        let unavailable = msg.contains("could not find any pods")
                            || msg.contains("no instances currently available");
                        if unavailable && iter.peek().is_some() {
                            warn!("not available, trying next GPU...");
                        } else if no_candidates {
                            return Err(e);
                        } else if iter.peek().is_none() {
                            anyhow::bail!("no available GPU found after trying all candidates: {e}");
                        } else {
                            return Err(e);
                        }
                    }
                }
            }
            let pod = pod.context("pod creation failed")?;
            let pod_id = pod.get("id").and_then(|v| v.as_str()).context("created pod missing id")?;
            info!("created pod: {pod_id}");
            if let Some(price) = chosen_price {
                info!("estimated cost: ${price:.2}/hr — pass --gpu-price {price:.2} to listen-test for run log");
            }
        }
        Command::Normalize { text } => {
            println!("{}", tts::f5::normalizer::normalize(&text));
        }
        Command::ListenTest {
            name,
            pod_id,
            speaker,
            cfg_scale,
            poll_secs,
            timeout_mins,
            seed,
            gpu_price,
        } => {
            let data_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/data");
            let input_path = format!("{data_dir}/{name}.txt");
            let normalized_path = format!("{data_dir}/{name}_normalized.txt");
            let wav_path = format!("{data_dir}/{name}_generated.wav");

            info!("normalizing {input_path}");
            let input = std::fs::read_to_string(&input_path)
                .with_context(|| format!("reading {input_path}"))?;
            let normalized: String = input
                .lines()
                .map(|line| tts::f5::normalizer::normalize(line))
                .collect::<Vec<_>>()
                .join("\n");
            std::fs::write(&normalized_path, normalized + "\n")
                .with_context(|| format!("writing {normalized_path}"))?;

            let pod = client.get_pod(&pod_id).await?;

            // Capture GPU info before inference for the run log.
            let gpu_info_cmd = "nvidia-smi --query-gpu=name,memory.total --format=csv,noheader 2>/dev/null || echo unknown";
            let gpu_info = run_output(&runpod::ssh_exec_command(&pod_id, &pod, gpu_info_cmd)?).unwrap_or_else(|_| "unknown".into());
            info!("GPU: {gpu_info}");
            let (gpu_name, gpu_vram_mb) = {
                let mut parts = gpu_info.splitn(2, ',');
                let n = parts.next().unwrap_or("unknown").trim().to_string();
                let v = parts.next().and_then(|s| s.trim().trim_end_matches(" MiB").parse::<u64>().ok());
                (n, v)
            };

            let remote_txt = format!("/workspace/VibeVoice/demo/{name}_normalized.txt");
            run(&runpod::scp_upload_command(&pod_id, &pod, &normalized_path, &remote_txt)?)?;

            let output_dir = format!("/workspace/output_{name}");
            let log_path = format!("/workspace/output_{name}.log");
            let seed_arg = seed.map(|s| format!("--seed {s}")).unwrap_or_default();
            info!("starting inference (cfg_scale={cfg_scale}, speaker={speaker}{seed_display})",
                seed_display = seed.map(|s| format!(", seed={s}")).unwrap_or_default());
            let start_cmd = format!(
                "cd /workspace/VibeVoice && \
                 rm -f {log_path} && \
                 nohup python3 demo/inference_from_file.py \
                   --model_path vibevoice/VibeVoice-1.5B \
                   --txt_path demo/{name}_normalized.txt \
                   --speaker_names {speaker} \
                   --cfg_scale {cfg_scale} \
                   {seed_arg} \
                   --output_dir {output_dir} > {log_path} 2>&1 < /dev/null & disown"
            );
            run(&runpod::ssh_exec_command(&pod_id, &pod, &start_cmd)?)?;

            info!("polling {log_path} every {poll_secs}s (timeout {timeout_mins}m)");
            let inference_start = std::time::Instant::now();
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_mins * 60);
            let mut seed_used: Option<u64> = seed;
            loop {
                if tokio::time::Instant::now() >= deadline {
                    anyhow::bail!("timed out waiting for inference after {timeout_mins} minutes");
                }
                tokio::time::sleep(std::time::Duration::from_secs(poll_secs)).await;
                let check_cmd = format!(
                    "test -f {log_path} && (grep -q 'RTF (Real' {log_path} && echo DONE || (grep -iq 'Traceback' {log_path} && echo ERROR || echo RUNNING)) || echo MISSING"
                );
                let status = run_output(&runpod::ssh_exec_command(&pod_id, &pod, &check_cmd)?)?;
                info!("status: {status}");
                match status.as_str() {
                    "DONE" => {
                        let elapsed = inference_start.elapsed();
                        info!("inference time: {:.1}s", elapsed.as_secs_f64());
                        let seed_cmd = format!("grep 'Seed used:' {log_path} || true");
                        let seed_line = run_output(&runpod::ssh_exec_command(&pod_id, &pod, &seed_cmd)?)?;
                        if !seed_line.is_empty() {
                            info!("{seed_line}");
                            // parse "Seed used: 12345"
                            if let Some(n) = seed_line.split_whitespace().last().and_then(|s| s.parse().ok()) {
                                seed_used = Some(n);
                            }
                        }
                        break;
                    }
                    "ERROR" => {
                        let tail_cmd = format!("tail -n 40 {log_path}");
                        let tail = run_output(&runpod::ssh_exec_command(&pod_id, &pod, &tail_cmd)?)?;
                        anyhow::bail!("inference failed:\n{tail}");
                    }
                    "MISSING" => anyhow::bail!("{log_path} not found on pod"),
                    _ => continue,
                }
            }

            info!("downloading wav to {wav_path}");
            let remote_wav = format!("{output_dir}/*.wav");
            run(&runpod::scp_download_command(&pod_id, &pod, &remote_wav, &wav_path)?)?;

            let mut audio_duration_secs: Option<f64> = None;
            let wall = inference_start.elapsed().as_secs_f64();
            if let Ok(meta) = hound::WavReader::open(&wav_path) {
                let spec = meta.spec();
                let dur = meta.len() as f64 / spec.sample_rate as f64 / spec.channels as f64;
                audio_duration_secs = Some(dur);
                info!("audio duration: {:.1}s  RTF: {:.2}x", dur, wall / dur);
            }

            // Append structured run log entry.
            let entry = serde_json::json!({
                "timestamp": chrono::Utc::now().to_rfc3339(),
                "segment": name,
                "pod_id": pod_id,
                "gpu_name": gpu_name,
                "gpu_vram_mb": gpu_vram_mb,
                "gpu_price_per_hr": gpu_price,
                "speaker": speaker,
                "cfg_scale": cfg_scale,
                "seed": seed_used,
                "inference_wall_secs": wall,
                "audio_duration_secs": audio_duration_secs,
                "rtf": audio_duration_secs.map(|d| wall / d),
            });
            if let Err(e) = append_run_log(entry) {
                warn!("failed to write run log: {e}");
            }

            info!("done: {wav_path}");
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
