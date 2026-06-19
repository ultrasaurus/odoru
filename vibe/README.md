# vibe

Goal: create tools to speed evaluation of vibe voice under consideration for
inclusion in Odoru

Two binaries:
- `vibe` — CLI for RunPod pod management and TTS helpers
- `vibe-service` — Axum HTTP service that runs on the pod, wraps VibeVoice
  inference, and exposes `/health`, `/jobs`, and `/log/:id`

Standalone Cargo workspace — separate `Cargo.lock`, doesn't affect the
root `odoru` build/Dockerfile.

## Setup

```
cp .env.example .env
```

Fill in:
- `RUNPOD_API_KEY` — from RunPod account settings
- `NETWORK_VOLUME_ID` — used as the default `--network-volume-id` for `new-pod`
- `$TEMPLATE` — default template id for `new-pod`
- `VIBE_SERVICE_SECRET` — shared secret for `vibe-service` auth; generate with
  `openssl rand -base64 32`. Set the same value in the RunPod template env
  (see `runpod.md`). Also set `RUNPOD_USER_API_KEY` in the template env so
  the idle watchdog can auto-stop the pod.

`ssh`/`download`/`listen-test-ssh` use `~/.ssh/runpod` to connect directly to
`root@<publicIp> -p <port>` (the pod's mapped port 22).

## Docker image

see [setup.md](setup.md)

## Usage

```
cargo run -- --help
cargo run -- <command> --help
```

### Typical workflow

```bash
# Start a pod (auto-selects cheapest >=24GB GPU)
# If multiple templates exist, pass the template id explicitly
cargo run -- new-pod gpu e6qma5uqam

# Synthesize a segment — polls /health, submits async job, polls until done,
# downloads wav. No proxy timeout risk.
cargo run -- synthesize authorship_seg01 <pod_id> --seed 71463 --gpu-price <price>

# Run multiple segments in sequence
for seg in seg01 seg02 seg03; do
  cargo run -- synthesize authorship_$seg <pod_id> --seed 71463 --gpu-price <price>
done

# The idle watchdog auto-stops the pod after 3 min of inactivity.
# To terminate immediately:
cargo run -- terminate-pod <pod_id>

# SSH fallback (when vibe-service is not running or for debugging)
cargo run -- listen-test-ssh authorship_seg01 <pod_id> --seed 71463 --gpu-price <price>
```

### vibe-service endpoints (on the pod)

All requests except `/health` require `Authorization: Bearer <VIBE_SERVICE_SECRET>`.
Base URL: `https://<pod_id>-3000.proxy.runpod.net`

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Returns `{"status":"ready","gpu":"..."}` once service is up |
| `/jobs` | POST | Body: `{"text","seed","speaker","cfg_scale","name?"}`. Returns `{"job_id","name"}` immediately; inference runs in background. |
| `/jobs/:job_id` | GET | Returns `{"status","seed?","wall_secs?","audio_secs?","rtf?","name?"}`. Status: `pending\|running\|done\|error`. |
| `/jobs/:job_id/wav` | GET | Returns WAV bytes when done; 404 if unknown, 409 if not yet done. Fetch-once: wav is freed from memory after download. |
| `/log/:request_id` | GET | Full stdout/stderr from the inference run. |
