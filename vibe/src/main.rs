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
