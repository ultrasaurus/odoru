mod config;
mod runpod;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

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
    /// none specified)
    NewPod {
        compute_type: runpod::ComputeType,
        template_id: Option<String>,
        /// Network volume to attach (defaults to $NETWORK_VOLUME_ID from vibe/.env)
        #[arg(long)]
        network_volume_id: Option<String>,
        /// Pod name
        #[arg(short, long, default_value = "vibevoice")]
        name: String,
        /// GPU type id (see `gpu-types`), e.g. "NVIDIA A40"
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
        #[arg(long, default_value_t = 2.0)]
        cfg_scale: f64,
        /// Seconds between checks of the inference log
        #[arg(long, default_value_t = 30)]
        poll_secs: u64,
        /// Give up waiting for inference after this many minutes
        #[arg(long, default_value_t = 60)]
        timeout_mins: u64,
    },
}

/// Run a command, streaming its output, and bail if it fails.
fn run(argv: &[String]) -> Result<()> {
    println!("running: {}", argv.join(" "));
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

#[tokio::main]
async fn main() -> Result<()> {
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
            println!("Terminated pod {pod_id}");
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
            println!("running: {}", argv.join(" "));
            let status = std::process::Command::new(&argv[0])
                .args(&argv[1..])
                .status()?;
            if !status.success() {
                anyhow::bail!("scp failed: {status}");
            }
            println!("downloaded to {local_path}");
        }
        Command::NewPod { compute_type, template_id, network_volume_id, name, gpu_type_id } => {
            let template_id = client.resolve_template(template_id).await?;
            println!("Using template: {template_id}");
            let network_volume_id = network_volume_id.or_else(|| std::env::var("NETWORK_VOLUME_ID").ok());
            let pod = client.create_pod(&template_id, compute_type, network_volume_id.as_deref(), &name, gpu_type_id.as_deref()).await?;
            let pod_id = pod.get("id").and_then(|v| v.as_str()).context("created pod missing id")?;
            println!("Created pod: {pod_id}");
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
        } => {
            let data_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/data");
            let input_path = format!("{data_dir}/{name}.txt");
            let normalized_path = format!("{data_dir}/{name}_normalized.txt");
            let wav_path = format!("{data_dir}/{name}_generated.wav");

            println!("normalizing {input_path}");
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

            let remote_txt = format!("/workspace/VibeVoice/demo/{name}_normalized.txt");
            run(&runpod::scp_upload_command(&pod_id, &pod, &normalized_path, &remote_txt)?)?;

            println!("ensuring vibevoice installed (/opt/venv)");
            let setup_cmd = "set -e; \
                /opt/venv/bin/python3 -c 'import vibevoice' 2>/dev/null || /opt/venv/bin/pip install -e /workspace/VibeVoice";
            run(&runpod::ssh_exec_command(&pod_id, &pod, setup_cmd)?)?;

            let output_dir = format!("/workspace/output_{name}");
            let log_path = format!("/workspace/output_{name}.log");
            println!("starting inference (cfg_scale={cfg_scale}, speaker={speaker})");
            let start_cmd = format!(
                "cd /workspace/VibeVoice && \
                 rm -f {log_path} && \
                 nohup /opt/venv/bin/python3 demo/inference_from_file.py \
                   --model_path vibevoice/VibeVoice-1.5B \
                   --txt_path demo/{name}_normalized.txt \
                   --speaker_names {speaker} \
                   --cfg_scale {cfg_scale} \
                   --output_dir {output_dir} > {log_path} 2>&1 < /dev/null & disown"
            );
            run(&runpod::ssh_exec_command(&pod_id, &pod, &start_cmd)?)?;

            println!("polling {log_path} every {poll_secs}s (timeout {timeout_mins}m)");
            let inference_start = std::time::Instant::now();
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_mins * 60);
            loop {
                if tokio::time::Instant::now() >= deadline {
                    anyhow::bail!("timed out waiting for inference after {timeout_mins} minutes");
                }
                tokio::time::sleep(std::time::Duration::from_secs(poll_secs)).await;
                let check_cmd = format!(
                    "test -f {log_path} && (grep -q 'RTF (Real' {log_path} && echo DONE || (grep -iq 'Traceback' {log_path} && echo ERROR || echo RUNNING)) || echo MISSING"
                );
                let status = run_output(&runpod::ssh_exec_command(&pod_id, &pod, &check_cmd)?)?;
                println!("status: {status}");
                match status.as_str() {
                    "DONE" => {
                        let elapsed = inference_start.elapsed();
                        println!("inference time: {:.1}s", elapsed.as_secs_f64());
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

            println!("downloading wav to {wav_path}");
            let remote_wav = format!("{output_dir}/*.wav");
            run(&runpod::scp_download_command(&pod_id, &pod, &remote_wav, &wav_path)?)?;

            // print audio duration and RTF
            if let Ok(meta) = hound::WavReader::open(&wav_path) {
                let spec = meta.spec();
                let duration_secs = meta.len() as f64 / spec.sample_rate as f64 / spec.channels as f64;
                let wall = inference_start.elapsed().as_secs_f64();
                println!("audio duration: {:.1}s  RTF: {:.2}x", duration_secs, wall / duration_secs);
            }

            println!("done: {wav_path}");
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
