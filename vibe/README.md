# vibe

Goal: create tools to speed evaluation of vibe voice under consideration for
inclusion in Odoru

Rust CLI for RunPod pod management and TTS helpers (replaces the old
SSH/curl one-liners).

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

`ssh`/`download` use `~/.ssh/runpod` to connect directly to
`root@<publicIp> -p <port>` (the pod's mapped port 22).

## Usage

```
cargo run -- --help
cargo run -- <command> --help
```
