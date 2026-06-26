//! `vibe synthesize` — runs VibeVoice inference via the vibe-service HTTP
//! API, either for a single pre-split segment or an explicit list/range of
//! segments submitted together as one batch (see dev/parallel.md "Stage 3
//! implementation plan").

use anyhow::{Context, Result};
use clap::Subcommand;
use tracing::{info, warn};

use crate::{runpod, segment};

/// What to synthesize: a pre-split segment file or a whole document.
#[derive(Subcommand)]
pub enum SynthInput {
    /// Synthesize a pre-split segment file: reads <basedir>/<name>.txt.
    Segment {
        /// Stem of <basedir>/<name>.txt (no extension, e.g. authorship_seg01)
        name: String,
    },
    /// Synthesize an explicit list/range of segments as one batch (POST
    /// /batches) — not a whole document. E.g. "augment_seg41-56" or
    /// "augment_seg41,augment_seg43,augment_seg50". See dev/parallel.md
    /// "Stage 3 implementation plan" for why this is list/range, not
    /// whole-doc: current workflow is iterative and listen-driven.
    Segments {
        /// Comma-separated names and/or "<prefix><N>-<M>" ranges.
        spec: String,
    },
    /// Segment a whole document and synthesize each part in sequence.
    /// Reads <basedir>/<name>.txt, writes <basedir>/<name>_seg*.txt, then
    /// synthesizes each segment in order.
    Doc {
        /// Stem of vibe/data/<name>.txt (no extension, e.g. authorship)
        name: String,
    },
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    client: &runpod::Client,
    input: SynthInput,
    pod_id: Option<String>,
    url: Option<String>,
    speaker: String,
    cfg_scale: f64,
    temp: Option<f64>,
    speed: Option<f64>,
    seed: u64,
    gpu_price: Option<f64>,
    port: u16,
    basedir: Option<String>,
) -> Result<()> {
    match input {
        SynthInput::Doc { .. } => {
            anyhow::bail!("synthesize doc is not yet implemented — run `segment <name>` first, then synthesize each segment");
        }
        SynthInput::Segment { name } => {
            let data_dir = segment::resolve_basedir(basedir.as_deref());
            let input_path = format!("{data_dir}/{name}.txt");
            let normalized_path = format!("{data_dir}/{name}_normalized.txt");

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

            let secret = std::env::var("VIBE_SERVICE_SECRET").ok();
            let http = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()?;
            let (synth_base_url, via, gpu_name, gpu_vram_mb) =
                resolve_synth_target(client, &http, &url, &pod_id, port).await?;

            // POST /jobs — returns immediately with job_id.
            info!("submitting job: {name} (seed={seed}, cfg={cfg_scale}, speaker={speaker}, temp={temp:?}, speed={speed:?})");
            let mut submit_req = http
                .post(format!("{synth_base_url}/jobs"))
                .json(&serde_json::json!({
                    "text": normalized,
                    "seed": seed,
                    "speaker": speaker,
                    "cfg_scale": cfg_scale,
                    "temp": temp,
                    "speed": speed,
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
            loop {
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
                        let elapsed = synth_start.elapsed().as_secs_f64();
                        info!("job_id={job_id_remote} name={name} status={status} elapsed={elapsed:.0}s");
                        match status {
                            "done" | "error" => break,
                            _ => continue,
                        }
                    }
                }
            }

            let (seed_used, wall, audio_duration_secs, rtf) =
                fetch_job_result(&http, &synth_base_url, &secret, &job_id_remote, &name, &data_dir).await?;
            if let Some(d) = audio_duration_secs {
                info!("audio: {d:.1}s  RTF: {:.2}x", rtf.unwrap_or(wall / d));
            }

            // Update the sidecar with this segment's output files and voice
            // (non-fatal — synthesis output is already saved regardless).
            segment::record_synthesis(&data_dir, &name, &speaker);

            // Append run log.
            let entry = serde_json::json!({
                "timestamp": chrono::Utc::now().to_rfc3339(),
                "segment": name,
                "pod_id": pod_id,
                "job_id": job_id_remote,
                "gpu_name": gpu_name,
                "gpu_vram_mb": gpu_vram_mb,
                "gpu_price_per_hr": gpu_price,
                "speaker": speaker,
                "cfg_scale": cfg_scale,
                "temp": temp,
                "speed": speed,
                "seed": seed_used,
                "inference_wall_secs": wall,
                "audio_duration_secs": audio_duration_secs,
                "rtf": rtf,
                "via": via,
            });
            if let Err(e) = append_run_log(entry) {
                warn!("failed to write run log: {e}");
            }
            info!("done: {data_dir}/{name}_generated.wav");
        }
        SynthInput::Segments { spec } => {
            let names = parse_segment_list(&spec)?;
            let data_dir = segment::resolve_basedir(basedir.as_deref());
            let secret = std::env::var("VIBE_SERVICE_SECRET").ok();
            let http = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()?;

            // Normalize each segment, building the batch payload.
            let mut segments_json = Vec::with_capacity(names.len());
            for name in &names {
                let input_path = format!("{data_dir}/{name}.txt");
                let normalized_path = format!("{data_dir}/{name}_normalized.txt");
                info!("normalizing {input_path}");
                let input = std::fs::read_to_string(&input_path)
                    .with_context(|| format!("reading {input_path}"))?;
                let normalized: String = input
                    .lines()
                    .map(|line| {
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
                segments_json.push(serde_json::json!({ "text": normalized, "name": name }));
            }

            let (synth_base_url, via, gpu_name, gpu_vram_mb) =
                resolve_synth_target(client, &http, &url, &pod_id, port).await?;

            info!(
                "submitting batch: {} segments (seed={seed}, cfg={cfg_scale}, speaker={speaker}, temp={temp:?}, speed={speed:?})",
                names.len()
            );
            let mut submit_req = http
                .post(format!("{synth_base_url}/batches"))
                .json(&serde_json::json!({
                    "segments": segments_json,
                    "seed": seed,
                    "speaker": speaker,
                    "cfg_scale": cfg_scale,
                    "temp": temp,
                    "speed": speed,
                }));
            if let Some(ref s) = secret {
                submit_req = submit_req.bearer_auth(s);
            }
            let submit_resp = submit_req.send().await.context("POST /batches")?;
            if !submit_resp.status().is_success() {
                let body = submit_resp.text().await.unwrap_or_default();
                anyhow::bail!("batch submission failed: {body}");
            }
            let batch: serde_json::Value = submit_resp.json().await.context("reading batch response")?;
            let batch_id = batch["batch_id"].as_str().context("missing batch_id in response")?.to_string();
            let job_ids: Vec<String> = batch["job_ids"]
                .as_array()
                .context("missing job_ids in response")?
                .iter()
                .map(|v| v.as_str().unwrap_or_default().to_string())
                .collect();
            info!("batch submitted: batch_id={batch_id} segments={}", names.len());

            // Poll GET /batches/:id once, not per-job — all segments in a
            // batch share fate (one generate() call), so there's no
            // staggered per-job completion to poll for separately (see
            // dev/parallel.md "Polling: once, not N times"). Falls back to
            // per-job polling if the batch_id isn't found (e.g. instance
            // churn — the in-memory batch map is an accepted gap; the
            // job_ids already in hand survive churn individually via the
            // durable store).
            let poll_client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()?;
            let batch_url = format!("{synth_base_url}/batches/{batch_id}");
            let synth_start = std::time::Instant::now();
            let mut batch_not_found = false;
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                let mut poll_req = poll_client.get(&batch_url);
                if let Some(ref s) = secret {
                    poll_req = poll_req.bearer_auth(s);
                }
                match poll_req.send().await {
                    Err(e) => {
                        warn!("poll batch_id={batch_id}: {e} — retrying");
                        continue;
                    }
                    Ok(r) if r.status() == reqwest::StatusCode::NOT_FOUND => {
                        warn!("batch_id={batch_id} not found (instance churn?) — falling back to per-job polling");
                        batch_not_found = true;
                        break;
                    }
                    Ok(r) if !r.status().is_success() => {
                        anyhow::bail!("GET /batches/{batch_id} returned HTTP {}", r.status());
                    }
                    Ok(r) => {
                        let j: serde_json::Value = r.json().await.context("reading batch status")?;
                        let status = j["status"].as_str().unwrap_or("unknown");
                        let elapsed = synth_start.elapsed().as_secs_f64();
                        info!("batch_id={batch_id} status={status} elapsed={elapsed:.0}s");
                        match status {
                            "done" => break,
                            "error" => {
                                let errored = j["errored_job_ids"].as_array().map(|a| a.len()).unwrap_or(0);
                                warn!("batch_id={batch_id}: {errored} segment(s) errored — fetching results for the rest individually");
                                break;
                            }
                            _ => continue,
                        }
                    }
                }
            }

            if batch_not_found {
                for job_id in &job_ids {
                    let job_url = format!("{synth_base_url}/jobs/{job_id}");
                    loop {
                        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                        let mut poll_req = poll_client.get(&job_url);
                        if let Some(ref s) = secret {
                            poll_req = poll_req.bearer_auth(s);
                        }
                        match poll_req.send().await {
                            Err(e) => {
                                warn!("poll job_id={job_id}: {e} — retrying");
                                continue;
                            }
                            Ok(r) if !r.status().is_success() => {
                                warn!("GET /jobs/{job_id} returned HTTP {} — giving up on this job", r.status());
                                break;
                            }
                            Ok(r) => {
                                let j: serde_json::Value = r.json().await.unwrap_or_default();
                                match j["status"].as_str().unwrap_or("unknown") {
                                    "done" | "error" => break,
                                    _ => continue,
                                }
                            }
                        }
                    }
                }
            }

            // Fetch + finalize each segment individually — reuses the
            // same per-job wav/transcript/report fetch path as the
            // single-segment branch above.
            for (job_id, name) in job_ids.iter().zip(&names) {
                match fetch_job_result(&http, &synth_base_url, &secret, job_id, name, &data_dir).await {
                    Ok((seed_used, wall, audio_secs, rtf)) => {
                        if let Some(d) = audio_secs {
                            info!("{name}: audio {d:.1}s  RTF: {:.2}x", rtf.unwrap_or(wall / d));
                        }
                        segment::record_synthesis(&data_dir, name, &speaker);
                        let entry = serde_json::json!({
                            "timestamp": chrono::Utc::now().to_rfc3339(),
                            "segment": name,
                            "pod_id": pod_id,
                            "job_id": job_id,
                            "batch_id": batch_id,
                            "gpu_name": gpu_name,
                            "gpu_vram_mb": gpu_vram_mb,
                            "gpu_price_per_hr": gpu_price,
                            "speaker": speaker,
                            "cfg_scale": cfg_scale,
                            "temp": temp,
                            "speed": speed,
                            "seed": seed_used,
                            "inference_wall_secs": wall,
                            "audio_duration_secs": audio_secs,
                            "rtf": rtf,
                            "via": via,
                        });
                        if let Err(e) = append_run_log(entry) {
                            warn!("failed to write run log for {name}: {e}");
                        }
                        info!("done: {data_dir}/{name}_generated.wav");
                    }
                    Err(e) => {
                        warn!("segment {name} (job_id={job_id}) failed: {e}");
                    }
                }
            }
        }
    }

    Ok(())
}

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

async fn fetch_align_results(
    http: &reqwest::Client,
    base_url: &str,
    job_id: &str,
    name: &str,
    data_dir: &str,
    secret: &Option<String>,
) {
    for (suffix, file_suffix) in [("transcript", "_transcript.json"), ("report", "_report.json")] {
        let url = format!("{base_url}/jobs/{job_id}/{suffix}");
        let mut req = http.get(&url).timeout(std::time::Duration::from_secs(30));
        if let Some(s) = secret {
            req = req.bearer_auth(s);
        }
        match req.send().await {
            Err(e) => {
                warn!("fetch alignment {suffix} for {name}: {e}");
                continue;
            }
            Ok(r) if !r.status().is_success() => {
                warn!("fetch alignment {suffix} for {name}: HTTP {}", r.status());
                continue;
            }
            Ok(r) => match r.bytes().await {
                Err(e) => warn!("reading alignment {suffix} for {name}: {e}"),
                Ok(bytes) => {
                    let path = format!("{data_dir}/{name}{file_suffix}");
                    if let Err(e) = std::fs::write(&path, &bytes) {
                        warn!("writing {path}: {e}");
                    } else {
                        // Print QA summary from report.
                        if suffix == "report" {
                            print_align_qa(name, &bytes);
                        }
                        info!("saved {path}");
                    }
                }
            },
        }
    }
}

/// Parses a comma-separated list/range spec into segment names, e.g.
/// "augment_seg41-43" -> [augment_seg41, augment_seg42, augment_seg43], or
/// "augment_seg41,augment_seg43,augment_seg50" -> those three literally.
/// Not a whole-document mode — explicit selection only (see
/// dev/parallel.md "Stage 3 implementation plan": current workflow is
/// iterative/listen-driven, not whole-doc renders, so the client needs to
/// pick a range or list, not assume "the whole doc").
fn parse_segment_list(spec: &str) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for token in spec.split(',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        names.extend(expand_range_token(token)?);
    }
    if names.is_empty() {
        anyhow::bail!("no segment names parsed from {spec:?}");
    }
    Ok(names)
}

/// Expands "<prefix><digits>-<digits>" (e.g. "augment_seg41-43") into the
/// individual names, zero-padded to match the width of the start number as
/// typed (so "seg01-09" pads to 2 digits, "seg100-105" doesn't pad). Tokens
/// that don't match this shape pass through unchanged as a single literal
/// name.
fn expand_range_token(token: &str) -> Result<Vec<String>> {
    if let Some(dash_pos) = token.rfind('-') {
        let (left, right) = (&token[..dash_pos], &token[dash_pos + 1..]);
        if !right.is_empty() && right.chars().all(|c| c.is_ascii_digit()) {
            let digit_start = left
                .rfind(|c: char| !c.is_ascii_digit())
                .map(|i| i + 1)
                .unwrap_or(0);
            let prefix = &left[..digit_start];
            let start_str = &left[digit_start..];
            if !start_str.is_empty() {
                let start: u32 = start_str.parse()?;
                let end: u32 = right.parse()?;
                let width = start_str.len();
                if start <= end {
                    return Ok((start..=end).map(|n| format!("{prefix}{n:0width$}")).collect());
                }
            }
        }
    }
    Ok(vec![token.to_string()])
}

/// Shared by both `SynthInput::Segment` and `SynthInput::Segments`: resolves
/// `--url`/`pod_id` to an actual reachable base URL, waits for `/health`,
/// then (RunPod only) looks up the pod's direct IP. Returns
/// (synth_base_url, via, gpu_name, gpu_vram_mb).
async fn resolve_synth_target(
    client: &runpod::Client,
    http: &reqwest::Client,
    url: &Option<String>,
    pod_id: &Option<String>,
    port: u16,
) -> Result<(String, &'static str, String, Option<u64>)> {
    let base_url = match url {
        Some(u) => u.clone(),
        None => {
            let pod_id = pod_id.as_deref().context("pod_id or --url is required")?;
            format!("https://{pod_id}-{port}.proxy.runpod.net")
        }
    };

    // Poll /health via proxy until ready (proxy URL works before the pod
    // IP/portMappings are populated, so we use it here).
    info!("waiting for vibe-service at {base_url}/health ...");
    let (gpu_name, gpu_vram_mb) = loop {
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
                    break (gpu_name, gpu_vram_mb);
                }
                if let Some(msg) = body.get("message").and_then(|v| v.as_str()) {
                    anyhow::bail!("service reported error: {msg}");
                }
            }
            Ok(r) => info!("health: HTTP {} — retrying", r.status()),
            Err(e) => info!("health: {e} — retrying"),
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    };

    // After health is confirmed, fetch pod details for direct IP — only
    // applies to RunPod; a --url target has no such concept.
    let (synth_base_url, via) = if url.is_some() {
        (base_url, "url")
    } else {
        let pod_id = pod_id.as_deref().context("pod_id or --url is required")?;
        let pod = client.get_pod(pod_id).await?;
        match runpod::http_direct_url(&pod, port) {
            Some(direct) => {
                info!("using direct IP: {direct}");
                (direct, "http-direct")
            }
            None => {
                warn!("no portMappings[{port}] found — falling back to proxy (may timeout on long segments)");
                (base_url, "http-proxy")
            }
        }
    };

    Ok((synth_base_url, via, gpu_name, gpu_vram_mb))
}

/// Fetches a completed job's status fields, wav, and alignment results —
/// shared finalization step for both the single-segment and batch synth
/// paths once a job_id is known to be done.
async fn fetch_job_result(
    http: &reqwest::Client,
    synth_base_url: &str,
    secret: &Option<String>,
    job_id: &str,
    name: &str,
    data_dir: &str,
) -> Result<(u64, f64, Option<f64>, Option<f64>)> {
    let mut status_req = http.get(format!("{synth_base_url}/jobs/{job_id}"));
    if let Some(s) = secret {
        status_req = status_req.bearer_auth(s);
    }
    let resp = status_req.send().await.context("GET /jobs/:id")?;
    if !resp.status().is_success() {
        anyhow::bail!("GET /jobs/{job_id} returned HTTP {}", resp.status());
    }
    let j: serde_json::Value = resp.json().await.context("reading job status")?;
    match j["status"].as_str().unwrap_or("unknown") {
        "done" => {}
        "error" => {
            let err = j["error"].as_str().unwrap_or("unknown error");
            anyhow::bail!("job {job_id} ({name}) failed: {err}");
        }
        other => anyhow::bail!("job {job_id} ({name}) not done yet (status={other})"),
    }
    let seed_used = j["seed"].as_u64().unwrap_or(0);
    let wall = j["wall_secs"].as_f64().unwrap_or(0.0);
    let audio_secs = j["audio_secs"].as_f64();
    let rtf = j["rtf"].as_f64();

    let wav_path = format!("{data_dir}/{name}_generated.wav");
    let mut wav_req = http.get(format!("{synth_base_url}/jobs/{job_id}/wav"))
        .timeout(std::time::Duration::from_secs(120));
    if let Some(s) = secret {
        wav_req = wav_req.bearer_auth(s);
    }
    let wav_resp = wav_req.send().await.context("GET /jobs/:id/wav")?;
    if !wav_resp.status().is_success() {
        anyhow::bail!("wav fetch failed: HTTP {}", wav_resp.status());
    }
    let wav_bytes = wav_resp.bytes().await.context("reading wav bytes")?;
    std::fs::write(&wav_path, &wav_bytes).with_context(|| format!("writing {wav_path}"))?;
    info!("saved wav to {wav_path} ({} bytes)", wav_bytes.len());

    fetch_align_results(http, synth_base_url, job_id, name, data_dir, secret).await;

    Ok((seed_used, wall, audio_secs, rtf))
}

fn print_align_qa(name: &str, report_bytes: &[u8]) {
    let Ok(report) = serde_json::from_slice::<segment::AlignReport>(report_bytes) else { return };

    if report.suspect.is_empty() && report.filtered.is_empty() {
        info!("QA {name}: clean");
        return;
    }

    let truncated = report.truncated();
    if !truncated.is_empty() {
        let words: Vec<_> = truncated.iter().map(|s| format!("{}({:.2})", s.word, s.score)).collect();
        warn!("QA {name}: ⚠ TRUNCATED — {}", words.join(" "));
    }
    let low = report.low_score();
    if !low.is_empty() {
        let words: Vec<_> = low.iter().map(|s| format!("{}({:.2})", s.word, s.score)).collect();
        warn!("QA {name}: low-score — {}", words.join(" "));
    }
    if !report.filtered.is_empty() {
        info!("QA {name}: {} filtered word(s)", report.filtered.len());
    }
}

#[cfg(test)]
mod segment_list_tests {
    use super::parse_segment_list;

    #[test]
    fn literal_comma_list() {
        let names = parse_segment_list("augment_seg41,augment_seg43,augment_seg50").unwrap();
        assert_eq!(names, vec!["augment_seg41", "augment_seg43", "augment_seg50"]);
    }

    #[test]
    fn range_expands_inclusive() {
        let names = parse_segment_list("augment_seg41-43").unwrap();
        assert_eq!(names, vec!["augment_seg41", "augment_seg42", "augment_seg43"]);
    }

    #[test]
    fn range_preserves_zero_padding_width() {
        let names = parse_segment_list("augment_seg08-10").unwrap();
        assert_eq!(names, vec!["augment_seg08", "augment_seg09", "augment_seg10"]);
    }

    #[test]
    fn range_and_literals_mixed() {
        let names = parse_segment_list("augment_seg41-43,augment_seg50").unwrap();
        assert_eq!(
            names,
            vec!["augment_seg41", "augment_seg42", "augment_seg43", "augment_seg50"]
        );
    }

    #[test]
    fn whitespace_around_tokens_is_trimmed() {
        let names = parse_segment_list(" augment_seg41 , augment_seg43 ").unwrap();
        assert_eq!(names, vec!["augment_seg41", "augment_seg43"]);
    }

    #[test]
    fn empty_spec_errors() {
        assert!(parse_segment_list("").is_err());
        assert!(parse_segment_list(" , ,").is_err());
    }
}
