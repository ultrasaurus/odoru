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

## Docker image

Build and push (must be run from the repo root, bump the version tag each
time — RunPod won't pull an updated image if the tag hasn't changed):

```
docker build --platform=linux/amd64 -f vibe/Dockerfile -t vibe:latest .
docker tag vibe:latest dockersaura/vibe:v4
docker push dockersaura/vibe:v4
```

Then update the RunPod template to point at the new tag before creating a
new pod.

## Usage

```
cargo run -- --help
cargo run -- <command> --help
```
