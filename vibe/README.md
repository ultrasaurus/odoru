# vibe

Goal: create tools to speed evaluation of vibe voice under consideration for
inclusion in Odoru

Two binaries:
- `vibe` — CLI for RunPod pod management and TTS helpers
- `vibe-service` — Axum HTTP service that runs on the pod, wraps VibeVoice
  inference, and exposes `/health`, `/synthesize`, and `/log/:id`

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
cargo run -- new-pod gpu

# Synthesize a segment — polls /health automatically, no manual wait needed
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
| `/synthesize` | POST | Body: `{"text","seed","speaker","cfg_scale"}`. Blocks until done. Returns WAV bytes + `X-Vibe-*` headers. |
| `/log/:request_id` | GET | Full stdout/stderr from the inference run. |

Response headers from `/synthesize`:
- `X-Vibe-Request-Id` — use to fetch the log
- `X-Vibe-Seed` — seed actually used
- `X-Vibe-Gpu` — GPU name from `nvidia-smi`
- `X-Vibe-Wall-Secs` — inference wall time in seconds
- `X-Vibe-Audio-Secs` — duration of generated audio in seconds
- `X-Vibe-Rtf` — real-time factor (wall / audio duration)
