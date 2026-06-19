use anyhow::{bail, Context, Result};
use serde_json::Value;

const BASE_URL: &str = "https://rest.runpod.io/v1";

/// Return `http://<publicIp>:<mapped_port>` for a direct HTTP connection to
/// a pod port, bypassing the RunPod proxy. Returns `None` if the pod has no
/// public IP or no mapping for the requested port.
pub fn http_direct_url(pod: &Value, port: u16) -> Option<String> {
    let ip = pod.get("publicIp").and_then(|v| v.as_str()).filter(|s| !s.is_empty())?;
    let mapped = pod.get("portMappings").and_then(|m| m.get(port.to_string())).and_then(|v| v.as_u64())?;
    Some(format!("http://{ip}:{mapped}"))
}

/// Get the (publicIp, sshPort) for a running pod's direct SSH access
/// (`root@<publicIp> -p <port>`, mapped from the pod's port 22).
fn ssh_endpoint(pod_id: &str, pod: &Value) -> Result<(String, u64)> {
    if pod.get("desiredStatus").and_then(|v| v.as_str()) != Some("RUNNING") {
        bail!("pod {pod_id} is not RUNNING — start it first");
    }

    let ip = pod
        .get("publicIp")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .with_context(|| format!("pod {pod_id} has no publicIp"))?
        .to_string();

    let port = pod
        .get("portMappings")
        .and_then(|v| v.get("22"))
        .and_then(|v| v.as_u64())
        .with_context(|| format!("pod {pod_id} has no port 22 mapping"))?;

    Ok((ip, port))
}

/// Build the SSH connection command for a running pod (direct
/// `root@<publicIp> -p <port>`, mapped from the pod's port 22).
pub fn ssh_command(pod_id: &str, pod: &Value) -> Result<String> {
    let (ip, port) = ssh_endpoint(pod_id, pod)?;
    Ok(format!("ssh -i ~/.ssh/runpod -p {port} root@{ip}"))
}

/// Build the `scp` command to download a file from a running pod.
pub fn scp_download_command(pod_id: &str, pod: &Value, remote_path: &str, local_path: &str) -> Result<Vec<String>> {
    let (ip, port) = ssh_endpoint(pod_id, pod)?;
    Ok(vec![
        "scp".to_string(),
        "-i".to_string(),
        shellexpand_home("~/.ssh/runpod"),
        "-P".to_string(),
        port.to_string(),
        format!("root@{ip}:{remote_path}"),
        local_path.to_string(),
    ])
}

/// Build the `scp` command to upload a file to a running pod.
pub fn scp_upload_command(pod_id: &str, pod: &Value, local_path: &str, remote_path: &str) -> Result<Vec<String>> {
    let (ip, port) = ssh_endpoint(pod_id, pod)?;
    Ok(vec![
        "scp".to_string(),
        "-i".to_string(),
        shellexpand_home("~/.ssh/runpod"),
        "-P".to_string(),
        port.to_string(),
        "-o".to_string(),
        "StrictHostKeyChecking=accept-new".to_string(),
        local_path.to_string(),
        format!("root@{ip}:{remote_path}"),
    ])
}

/// Build an `ssh` command that runs `remote_cmd` via `bash -lc` on a
/// running pod and returns (does not open an interactive shell).
pub fn ssh_exec_command(pod_id: &str, pod: &Value, remote_cmd: &str) -> Result<Vec<String>> {
    let (ip, port) = ssh_endpoint(pod_id, pod)?;
    Ok(vec![
        "ssh".to_string(),
        "-i".to_string(),
        shellexpand_home("~/.ssh/runpod"),
        "-p".to_string(),
        port.to_string(),
        "-o".to_string(),
        "StrictHostKeyChecking=accept-new".to_string(),
        format!("root@{ip}"),
        "--".to_string(),
        format!("bash -lc {}", shell_quote(remote_cmd)),
    ])
}

/// Single-quote a string for a POSIX shell, escaping embedded `'`.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn shellexpand_home(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    path.to_string()
}

/// Turn a response into JSON, including the body text in the error
/// on non-2xx status (RunPod's error responses are JSON with useful
/// `"error"` messages, e.g. "not enough free GPUs").
async fn into_json(resp: reqwest::Response) -> Result<Value> {
    let status = resp.status();
    let body = resp.text().await.context("reading response body")?;

    if !status.is_success() {
        bail!("HTTP {status}: {body}");
    }

    serde_json::from_str(&body).with_context(|| format!("parsing response: {body}"))
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum ComputeType {
    Cpu,
    Gpu,
}

pub struct Client {
    api_key: String,
    http: reqwest::Client,
}

impl Client {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            http: reqwest::Client::new(),
        }
    }

    pub async fn list_templates(&self) -> Result<Value> {
        let resp = self
            .http
            .get(format!("{BASE_URL}/templates"))
            .bearer_auth(&self.api_key)
            .send()
            .await
            .context("requesting templates")?;
        into_json(resp).await
    }

    pub async fn get_pod(&self, pod_id: &str) -> Result<Value> {
        let resp = self
            .http
            .get(format!("{BASE_URL}/pods/{pod_id}"))
            .bearer_auth(&self.api_key)
            .send()
            .await
            .context("requesting pod")?;
        into_json(resp).await
    }

    pub async fn start_pod(&self, pod_id: &str) -> Result<Value> {
        self.pod_action(pod_id, "start").await
    }

    pub async fn stop_pod(&self, pod_id: &str) -> Result<Value> {
        self.pod_action(pod_id, "stop").await
    }

    async fn pod_action(&self, pod_id: &str, action: &str) -> Result<Value> {
        let resp = self
            .http
            .post(format!("{BASE_URL}/pods/{pod_id}/{action}"))
            .bearer_auth(&self.api_key)
            .send()
            .await
            .with_context(|| format!("requesting pod {action}"))?;
        into_json(resp).await
    }

    /// If `template_id` is None, requires there to be exactly one
    /// template and uses it (mirrors `pod-lifecycle/dev/new-pod.sh`).
    pub async fn resolve_template(&self, template_id: Option<String>) -> Result<String> {
        if let Some(id) = template_id {
            return Ok(id);
        }

        let templates = self.list_templates().await?;
        let templates = templates
            .as_array()
            .context("templates response was not an array")?;

        match templates.len() {
            1 => templates[0]
                .get("id")
                .and_then(|v| v.as_str())
                .map(String::from)
                .context("template missing id"),
            0 => bail!("no templates found; pass a template_id"),
            n => {
                let names: Vec<String> = templates
                    .iter()
                    .map(|t| {
                        format!(
                            "{} ({})",
                            t.get("name").and_then(|v| v.as_str()).unwrap_or("?"),
                            t.get("id").and_then(|v| v.as_str()).unwrap_or("?")
                        )
                    })
                    .collect();
                bail!("expected exactly one template, found {n}: {}", names.join(", "))
            }
        }
    }

    /// Create a new pod from a template. For CPU this matches
    /// `pod-lifecycle/dev/new-pod.sh`; for GPU we pick `gpuCount: 1`
    /// and leave RunPod to choose a GPU type.
    pub async fn create_pod(
        &self,
        template_id: &str,
        compute_type: ComputeType,
        network_volume_id: Option<&str>,
        name: &str,
        gpu_type_id: Option<&str>,
    ) -> Result<Value> {
        let mut body = match compute_type {
            ComputeType::Cpu => serde_json::json!({
                "templateId": template_id,
                "name": name,
                "computeType": "CPU",
                "cpuFlavorIds": ["cpu3g"],
            }),
            ComputeType::Gpu => serde_json::json!({
                "templateId": template_id,
                "name": name,
                "computeType": "GPU",
                "gpuCount": 1,
            }),
        };

        if let Some(gpu_type_id) = gpu_type_id {
            body["gpuTypeIds"] = serde_json::json!([gpu_type_id]);
        }

        if let Some(volume_id) = network_volume_id {
            body["networkVolumeId"] = Value::from(volume_id);
        }

        let resp = self
            .http
            .post(format!("{BASE_URL}/pods"))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .context("requesting pod creation")?;
        into_json(resp).await
    }

    /// List GPU types with displayName/memory/lowest price via the
    /// GraphQL API (the REST API has no equivalent endpoint).
    pub async fn list_gpu_types(&self) -> Result<Value> {
        let query = r#"query {
            gpuTypes {
                id
                displayName
                memoryInGb
                secureCloud
                communityCloud
                lowestPrice(input: {gpuCount: 1}) { uninterruptablePrice }
            }
        }"#;

        let resp = self
            .http
            .post(format!("https://api.runpod.io/graphql?api_key={}", self.api_key))
            .json(&serde_json::json!({ "query": query }))
            .send()
            .await
            .context("requesting gpu types")?;

        let body = into_json(resp).await?;
        body.get("data")
            .and_then(|d| d.get("gpuTypes"))
            .cloned()
            .context("missing data.gpuTypes in response")
    }

    /// Terminate (delete) a pod entirely.
    pub async fn terminate_pod(&self, pod_id: &str) -> Result<()> {
        let resp = self
            .http
            .delete(format!("{BASE_URL}/pods/{pod_id}"))
            .bearer_auth(&self.api_key)
            .send()
            .await
            .context("requesting pod termination")?;

        let status = resp.status();
        let body = resp.text().await.context("reading response body")?;
        if !status.is_success() {
            bail!("HTTP {status}: {body}");
        }
        Ok(())
    }

    pub async fn list_pods(&self) -> Result<Value> {
        let resp = self
            .http
            .get(format!("{BASE_URL}/pods"))
            .bearer_auth(&self.api_key)
            .send()
            .await
            .context("requesting pods")?;
        into_json(resp).await
    }
}
