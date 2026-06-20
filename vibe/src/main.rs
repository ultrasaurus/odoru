mod config;
mod runpod;
mod watchdog;

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
    /// Reads vibe/data/<name>.txt, writes vibe/data/<name>_seg01.txt etc.
    /// with Speaker 1: prefix per paragraph.
    Segment {
        /// Stem of vibe/data/<name>.txt (no extension)
        name: String,
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
        pod_id: String,
        #[arg(long, default_value = "Sarah")]
        speaker: String,
        #[arg(long, default_value_t = 1.3)]
        cfg_scale: f64,
        /// Random seed (default: 71463)
        #[arg(long, default_value_t = 71463)]
        seed: u64,
        /// GPU price per hour (from new-pod output), stored in run log
        #[arg(long)]
        gpu_price: Option<f64>,
        /// Override the service port (default: 3000)
        #[arg(long, default_value_t = 3000)]
        port: u16,
    },
    /// Run the full listen-test loop via SSH/SCP (fallback when vibe-service
    /// is not running). Normalize data/<name>.txt, upload, run VibeVoice
    /// inference on the pod, wait for completion, and download the resulting
    /// wav to data/<name>_generated.wav.
    ListenTestSsh {
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

/// What to synthesize: a pre-split segment file or a whole document.
#[derive(Subcommand)]
enum SynthInput {
    /// Synthesize a pre-split segment file: reads vibe/data/<name>.txt.
    Segment {
        /// Stem of vibe/data/<name>.txt (no extension, e.g. authorship_seg01)
        name: String,
    },
    /// Segment a whole document and synthesize each part in sequence.
    /// Reads vibe/data/<name>.txt, writes vibe/data/<name>_seg*.txt, then
    /// synthesizes each segment in order.
    Doc {
        /// Stem of vibe/data/<name>.txt (no extension, e.g. authorship)
        name: String,
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

/// Append a JSON line to runs.jsonl for later cost/perf analysis.
fn append_run_log(entry: serde_json::Value) -> Result<()> {
    let vibe_dir = env!("CARGO_MANIFEST_DIR");
    let path = format!("{vibe_dir}/runs.jsonl");
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
            println!("{}", util::normalizer::normalize(&text));
        }
        Command::Segment { name } => {
            // Source documents live in the workspace data/ dir (odoru/data/).
            // Segment files are written to vibe/data/ for synthesis.
            let src_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../data");
            let seg_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/data");
            let input_path = format!("{src_dir}/{name}.txt");
            let text = std::fs::read_to_string(&input_path)
                .with_context(|| format!("reading {input_path}"))?;
            let segments = util::segmenter::segment(&text);
            info!("{} → {} segments", name, segments.len());
            for (i, seg) in segments.iter().enumerate() {
                let seg_name = format!("{name}_seg{:02}", i + 1);
                let seg_path = format!("{seg_dir}/{seg_name}.txt");
                let content: String = seg.lines()
                    .map(|p| format!("Speaker 1: {p}\n"))
                    .collect();
                std::fs::write(&seg_path, &content)
                    .with_context(|| format!("writing {seg_path}"))?;
                let wc: usize = seg.split_whitespace().count();
                info!("  {seg_name}: {} paragraphs, {wc} words", seg.lines().count());
            }
        }
        Command::Synthesize {
            input,
            pod_id,
            speaker,
            cfg_scale,
            seed,
            gpu_price,
            port,
        } => {
            let name = match &input {
                SynthInput::Segment { name } => name.clone(),
                SynthInput::Doc { .. } => {
                    anyhow::bail!("synthesize doc is not yet implemented — run `segment <name>` first, then synthesize each segment");
                }
            };
            let data_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/data");
            let input_path = format!("{data_dir}/{name}.txt");
            let normalized_path = format!("{data_dir}/{name}_normalized.txt");
            let wav_path = format!("{data_dir}/{name}_generated.wav");

            info!("normalizing {input_path}");
            let input = std::fs::read_to_string(&input_path)
                .with_context(|| format!("reading {input_path}"))?;
            let normalized: String = input
                .lines()
                .map(|line| {
                    // Preserve "Speaker N: " prefix — normalize only the content.
                    if let Some(rest) = line.strip_prefix("Speaker 1: ") {
                        format!("Speaker 1: {}", util::normalizer::normalize(rest))
                    } else {
                        util::normalizer::normalize(line)
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            std::fs::write(&normalized_path, normalized.clone() + "\n")
                .with_context(|| format!("writing {normalized_path}"))?;

            let proxy_base_url = format!("https://{pod_id}-{port}.proxy.runpod.net");
            let secret = std::env::var("VIBE_SERVICE_SECRET").ok();

            // Poll /health via proxy until ready (proxy URL works before the pod
            // IP/portMappings are populated, so we use it here).
            info!("waiting for vibe-service at {proxy_base_url}/health ...");
            let http = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()?;
            loop {
                match http.get(format!("{proxy_base_url}/health")).send().await {
                    Ok(r) if r.status().is_success() => {
                        let body: serde_json::Value = r.json().await.unwrap_or_default();
                        if body.get("status").and_then(|v| v.as_str()) == Some("ready") {
                            let gpu = body.get("gpu").and_then(|v| v.as_str()).unwrap_or("unknown");
                            info!("service ready — GPU: {gpu}");
                            break;
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

            // After health is confirmed, fetch pod details for direct IP.
            let pod = client.get_pod(&pod_id).await?;
            let (synth_base_url, via) = match runpod::http_direct_url(&pod, port) {
                Some(direct) => {
                    info!("using direct IP: {direct}");
                    (direct, "http-direct")
                }
                None => {
                    warn!("no portMappings[{port}] found — falling back to proxy (may timeout on long segments)");
                    (proxy_base_url, "http-proxy")
                }
            };

            // POST /jobs — returns immediately with job_id.
            info!("submitting job: {name} (seed={seed}, cfg={cfg_scale}, speaker={speaker})");
            let mut submit_req = http
                .post(format!("{synth_base_url}/jobs"))
                .json(&serde_json::json!({
                    "text": normalized,
                    "seed": seed,
                    "speaker": speaker,
                    "cfg_scale": cfg_scale,
                    "name": name,
                }));
            if let Some(ref s) = secret {
                submit_req = submit_req.bearer_auth(s);
            }
            let submit_resp = submit_req.send().await.context("POST /jobs")?;
            if !submit_resp.status().is_success() {
                let body = submit_resp.text().await.unwrap_or_default();
                anyhow::bail!("job submission failed: {body}");
            }
            let job: serde_json::Value = submit_resp.json().await.context("reading job response")?;
            let job_id_remote = job["job_id"].as_str().context("missing job_id in response")?.to_string();
            info!("job submitted: job_id={job_id_remote} name={name}");

            // Poll GET /jobs/:id until done or error.
            let poll_client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()?;
            let job_url = format!("{synth_base_url}/jobs/{job_id_remote}");
            let synth_start = std::time::Instant::now();
            let (seed_used, wall, audio_duration_secs, rtf) = loop {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                let mut poll_req = poll_client.get(&job_url);
                if let Some(ref s) = secret {
                    poll_req = poll_req.bearer_auth(s);
                }
                match poll_req.send().await {
                    Err(e) => {
                        warn!("poll job_id={job_id_remote} name={name}: {e} — retrying");
                        continue;
                    }
                    Ok(r) if !r.status().is_success() => {
                        anyhow::bail!("GET /jobs/{job_id_remote} returned HTTP {}", r.status());
                    }
                    Ok(r) => {
                        let j: serde_json::Value = r.json().await.context("reading job status")?;
                        let status = j["status"].as_str().unwrap_or("unknown");
                        info!("job_id={job_id_remote} name={name} status={status}");
                        match status {
                            "done" => {
                                let seed_used = j["seed"].as_u64().unwrap_or(seed);
                                let wall = j["wall_secs"].as_f64()
                                    .unwrap_or_else(|| synth_start.elapsed().as_secs_f64());
                                let audio_secs = j["audio_secs"].as_f64();
                                let rtf = j["rtf"].as_f64();
                                break (seed_used, wall, audio_secs, rtf);
                            }
                            "error" => {
                                let err = j["error"].as_str().unwrap_or("unknown error");
                                anyhow::bail!("job failed: {err}");
                            }
                            _ => continue,
                        }
                    }
                }
            };

            // Fetch wav.
            let mut wav_req = http.get(format!("{synth_base_url}/jobs/{job_id_remote}/wav"))
                .timeout(std::time::Duration::from_secs(120));
            if let Some(ref s) = secret {
                wav_req = wav_req.bearer_auth(s);
            }
            let wav_resp = wav_req.send().await.context("GET /jobs/:id/wav")?;
            if !wav_resp.status().is_success() {
                anyhow::bail!("wav fetch failed: HTTP {}", wav_resp.status());
            }
            let wav_bytes = wav_resp.bytes().await.context("reading wav bytes")?;
            std::fs::write(&wav_path, &wav_bytes).with_context(|| format!("writing {wav_path}"))?;
            info!("saved wav to {wav_path} ({} bytes)", wav_bytes.len());
            if let Some(d) = audio_duration_secs {
                info!("audio: {d:.1}s  RTF: {:.2}x", rtf.unwrap_or(wall / d));
            }

            // Append run log.
            let entry = serde_json::json!({
                "timestamp": chrono::Utc::now().to_rfc3339(),
                "segment": name,
                "pod_id": pod_id,
                "job_id": job_id_remote,
                "gpu_price_per_hr": gpu_price,
                "speaker": speaker,
                "cfg_scale": cfg_scale,
                "seed": seed_used,
                "inference_wall_secs": wall,
                "audio_duration_secs": audio_duration_secs,
                "rtf": rtf,
                "via": via,
            });
            if let Err(e) = append_run_log(entry) {
                warn!("failed to write run log: {e}");
            }
            info!("done: {wav_path}");
        }
        Command::ListenTestSsh {
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
                .map(|line| {
                    // Preserve "Speaker N: " prefix — normalize only the content.
                    if let Some(rest) = line.strip_prefix("Speaker 1: ") {
                        format!("Speaker 1: {}", util::normalizer::normalize(rest))
                    } else {
                        util::normalizer::normalize(line)
                    }
                })
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

            // Spawn watch-pod in the background so the user gets a macOS
            // notification if the pod is still running after the test ends.
            let exe = std::env::current_exe().unwrap_or_else(|_| "vibe".into());
            match std::process::Command::new(&exe)
                .args(["watch-pod", &pod_id])
                .spawn()
            {
                Ok(_) => info!("watch-pod spawned for {pod_id}"),
                Err(e) => warn!("failed to spawn watch-pod: {e}"),
            }
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
