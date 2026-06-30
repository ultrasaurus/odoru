//! `vibe synthesize` — runs VibeVoice inference via the vibe-service HTTP
//! API, either for a single pre-split segment or an explicit list/range of
//! segments submitted together as one batch (see dev/parallel.md "Stage 3
//! implementation plan").

use anyhow::{Context, Result};
use clap::Subcommand;
use tracing::{info, warn};

use crate::{runpod, segment, voice};

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
    /// "augment_seg41,augment_seg43,augment_seg50".
    Segments {
        /// Comma-separated names and/or "<prefix><N>-<M>" ranges.
        spec: String,
    },
    /// Export a document's text from Odoru, segment it, synthesize all
    /// segments as one batch, then import the result back into Odoru.
    /// Expects a fresh --basedir (errors if <basedir>/<name>.txt exists).
    #[command(after_help = "Note: --url/--basedir/--voice/etc. are options of \
        `synthesize`, not of `doc` — they go before the subcommand:\n\n  \
        vibe synthesize --url <URL> --basedir <DIR> doc <NAME> <DOC_ID>")]
    Doc {
        /// Stem of <basedir>/<name>.txt (no extension, e.g. authorship)
        name: String,
        /// Odoru document id to export from and import into
        doc_id: String,
        /// Resume an existing run: skip export/segment, synthesize only
        /// segments that don't yet have audio in --basedir, then import.
        /// Errors if --basedir doesn't already contain a prior `doc` run.
        #[arg(long)]
        missing: bool,
    },
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    client: &runpod::Client,
    input: SynthInput,
    pod_id: Option<String>,
    url: Option<String>,
    speaker: String,
    voice_name: Option<String>,
    cfg_scale: Option<f64>,
    temp: Option<f64>,
    speed: Option<f64>,
    seed: Option<u64>,
    gpu_price: Option<f64>,
    port: u16,
    basedir: Option<String>,
) -> Result<()> {
    // --voice supplies the speaker name plus cfg_scale/seed/speed/temp
    // defaults from its voice.md; explicit CLI flags still take precedence.
    let voice_def = voice_name
        .as_deref()
        .map(voice::VibeVoiceDef::load_named)
        .transpose()?;
    let speaker = voice_def.as_ref().map(|v| v.name.clone()).unwrap_or(speaker);
    let cfg_scale = cfg_scale.or(voice_def.as_ref().and_then(|v| v.cfg_scale)).unwrap_or(1.3);
    let seed = seed.or(voice_def.as_ref().and_then(|v| v.seed)).unwrap_or(71463);
    let speed = speed.or(voice_def.as_ref().and_then(|v| v.speed));
    let temp = temp.or(voice_def.as_ref().and_then(|v| v.temp));

    match input {
        SynthInput::Doc { name, doc_id, missing } => {
            crate::doc::run(
                client, name, doc_id, missing, voice_def.as_ref(), pod_id, url, speaker,
                cfg_scale, temp, speed, seed, gpu_price, port, basedir,
            )
            .await?;
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
            if let Some(v) = &voice_def {
                upload_voice_def(&http, &synth_base_url, v, &secret).await?;
            }

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
            run_segments_batch(
                client, voice_def.as_ref(), spec, pod_id, url, speaker, cfg_scale, temp, speed,
                seed, gpu_price, port, basedir,
            )
            .await?;
        }
    }

    Ok(())
}

/// Submit a list/range of segments as one batch (POST /batches), poll for
/// completion, and fetch + finalize each segment's result. Shared by
/// `SynthInput::Segments` and [`crate::doc::run`], which both need to
/// synthesize an already-known set of segments without going back through
/// [`run`]'s top-level `SynthInput` dispatch (that would recurse for `doc`).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_segments_batch(
    client: &runpod::Client,
    voice_def: Option<&voice::VibeVoiceDef>,
    spec: String,
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
    if let Some(v) = &voice_def {
        upload_voice_def(&http, &synth_base_url, v, &secret).await?;
    }

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

/// Compresses a sorted-or-not list of segment indices into a comma-joined
/// range spec parseable by [`parse_segment_list`], e.g. `[5, 6, 7, 9]` with
/// prefix "authorship_seg" -> "authorship_seg05-07,authorship_seg09".
/// Indices are deduplicated; each number is zero-padded to at least 2
/// digits, matching `segment::run`'s `<name>_seg{:02}` naming convention.
pub(crate) fn compress_segment_indices(prefix: &str, indices: &[u32]) -> String {
    let mut sorted: Vec<u32> = indices.to_vec();
    sorted.sort_unstable();
    sorted.dedup();

    let mut runs: Vec<(u32, u32)> = Vec::new();
    for n in sorted {
        match runs.last_mut() {
            Some((_, end)) if n == *end + 1 => *end = n,
            _ => runs.push((n, n)),
        }
    }

    runs.iter()
        .map(|(start, end)| {
            if start == end {
                format!("{prefix}{start:02}")
            } else {
                format!("{prefix}{start:02}-{end:02}")
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Read `voice_def.wav_path` and upload it to the resolved service URL,
/// so a `--voice`-selected voice is available before the job is submitted.
async fn upload_voice_def(
    http: &reqwest::Client,
    synth_base_url: &str,
    voice_def: &voice::VibeVoiceDef,
    secret: &Option<String>,
) -> Result<()> {
    let bytes = std::fs::read(&voice_def.wav_path)
        .with_context(|| format!("reading {}", voice_def.wav_path.display()))?;
    info!("uploading voice {} ({} bytes)", voice_def.name, bytes.len());
    voice::upload_wav(http, synth_base_url, &voice_def.name, &voice_def.gender, bytes, secret).await
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
    let (gpu_name, gpu_vram_mb) = crate::wait_for_health(http, &base_url).await?;

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

#[cfg(test)]
mod compress_segment_indices_tests {
    use super::compress_segment_indices;

    #[test]
    fn all_contiguous_collapses_to_one_range() {
        let spec = compress_segment_indices("authorship_seg", &[5, 6, 7, 8]);
        assert_eq!(spec, "authorship_seg05-08");
    }

    #[test]
    fn single_gap_splits_into_two_runs() {
        let spec = compress_segment_indices("authorship_seg", &[5, 6, 7, 9]);
        assert_eq!(spec, "authorship_seg05-07,authorship_seg09");
    }

    #[test]
    fn multiple_runs() {
        let spec = compress_segment_indices("augment_seg", &[1, 2, 3, 10, 12, 13, 14, 20]);
        assert_eq!(spec, "augment_seg01-03,augment_seg10,augment_seg12-14,augment_seg20");
    }

    #[test]
    fn unsorted_and_duplicate_indices_are_normalized() {
        let spec = compress_segment_indices("doc_seg", &[3, 1, 2, 2, 1]);
        assert_eq!(spec, "doc_seg01-03");
    }

    #[test]
    fn single_index() {
        let spec = compress_segment_indices("doc_seg", &[42]);
        assert_eq!(spec, "doc_seg42");
    }

    #[test]
    fn empty_indices_produces_empty_string() {
        let spec = compress_segment_indices("doc_seg", &[]);
        assert_eq!(spec, "");
    }

    #[test]
    fn round_trips_through_parse_segment_list() {
        let spec = compress_segment_indices("authorship_seg", &[5, 6, 7, 9]);
        let names = super::parse_segment_list(&spec).unwrap();
        assert_eq!(
            names,
            vec!["authorship_seg05", "authorship_seg06", "authorship_seg07", "authorship_seg09"]
        );
    }
}
