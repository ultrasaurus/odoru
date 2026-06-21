# Setup notes: pod + VibeVoice

## Docker image

VibeVoice and both HuggingFace models are baked into the image — no manual
pip install or model download needed on the pod.

Base image: `runpod/pytorch:2.4.0-py3.11-cuda12.4.1-devel-ubuntu22.04`
(PyTorch 2.4, CUDA 12.4, supports sm_50–sm_90 / all pre-Blackwell GPUs)

We tried `runpod/pytorch:1.0.6-cu1281-torch271-ubuntu2204` (PyTorch 2.7,
CUDA 12.8) to get Blackwell (sm_120) support, but most RunPod machines
have drivers too old for CUDA 12.8 and the container fails to start:
`nvidia-container-cli: requirement error: unsatisfied condition: cuda>=12.8`.
CUDA 12.4 works on a much wider range of machines.

### Docker image build
Build and push from the **repo root** (bump version tag each time — RunPod
won't re-pull if the tag is unchanged):

These are build instructions for the *next* version
```
VERSION=v15
docker build --platform=linux/amd64 -f vibe/Dockerfile -t vibe:latest .
docker tag vibe:latest dockersaura/vibe:$VERSION
docker push dockersaura/vibe:$VERSION
```

Current image: v14  (updated manually, check DockerHub to be sure)

After pushing, update the RunPod template via the PATCH curl in
`runpod.md`. We keep current template in: `$TEMPLATE`

The template must include `containerRegistryAuthId` for DockerHub auth —
without it, pulls fail with `IMAGE_AUTH_ERROR: toomanyrequests`.
Auth ID: `cmqi6vbaq003mq3m4cb5bs2bl` (already set in current template).

The `vibe-service` binary is compiled as a musl static binary (no glibc
or libssl deps) and copied into the image. It runs in the foreground
alongside sshd and exposes port 3000.

## Starting a pod

Multiple templates exist — always pass the template ID explicitly:

```
cargo run -- new-pod gpu e6qma5uqam
```

Auto-selects cheapest available GPU with **>=24GB VRAM** and retries down
the price list on "not available" errors. Prints GPU, price, and pod ID.

No need to wait after `new-pod` — `synthesize` polls `/health`
automatically until the service is ready.

No network volume — attaching one locks the pod to a specific datacenter
region, limiting GPU availability. Generated audio is downloaded locally
after each run. **Do not restart a stopped pod** — terminate and create a
new one instead. If you try anyway, expect `start-pod` to fail with
`HTTP 500: "There are not enough free GPUs on the host machine to start
this pod."` — the host machine reallocates the GPU once a pod exits, so
there's nothing to resume even if the UI still shows the old template/image.

SSH connects via direct IP+port (not the `<pod_id>@ssh.runpod.io` proxy):

```
cargo run -- ssh <pod_id>   # prints the ssh command
```

Terminate when done to stop billing (or let the idle watchdog do it):

```
cargo run -- terminate-pod <pod_id>
```

## Synthesizing segments

```
cargo run -- synthesize <pod_id> [--seed N] [--gpu-price P] segment <segment_name>
```

- Normalizes `vibe/data/<segment_name>.txt`
- Polls `/health` until `vibe-service` is ready (no manual wait needed)
- POSTs to `/synthesize` and blocks until inference completes
- Saves `vibe/data/<segment_name>_generated.wav`
- Fetches and saves the inference log to `vibe/data/<segment_name>_<id>.log`
- Appends a row to `vibe/runs.jsonl`

Pass `--seed <N>` to fix the voice across multiple segments. Preferred seed
is **71463** — see [voices.md](voices.md) for seed evaluations.

The idle watchdog in `vibe-service` auto-stops the pod after 3 minutes of
inactivity — no need to manually terminate after a run completes.

### Running multiple segments

```bash
for seg in seg01 seg02 seg03; do
  cargo run -- synthesize <pod_id> --seed 71463 --gpu-price <price> segment augment_$seg
done
```

## Segment files

Generate segment files with the `util::segmenter` logic, exposed via the
vibe CLI:

```bash
cargo run -- segment authorship
```

This reads `odoru/data/<name>.txt` and writes `vibe/data/<name>_seg01.txt`
… `<name>_segNN.txt` (50–250 words each, `Speaker 1: ` prefix per
paragraph). Short headings/fragments are merged into the following
paragraph — see `util/src/segmenter.rs` for the full pipeline and
`dev/plan.md` for the rationale.

Long inline quotes/parentheticals were briefly split out into their own
paragraphs (so the segmenter had finer break points), but that produced
TTS artifacts when a quote/paren delimiter landed alone at a line edge, so
it was reverted — quoted/parenthesized text now just stays inline with the
rest of its paragraph.

The segment count and numbering can shift whenever the segmenter logic
changes (e.g. authorship.txt currently produces 35 segments, not the
26 from an earlier splitting approach) — don't hardcode segment numbers
in docs or scripts; regenerate and check `cargo run -- segment <name>`
output for the current count.

**Archiving runs**: once a full run's audio has been reviewed, the
working files in `vibe/data/` (segment `.txt`/`.wav`/`.log`/normalized
files, concat list) get moved into a dated or purpose-named subdirectory
(e.g. `vibe/data/authorship-full-doc-1/`) to keep the top level clean for
the next run in progress. When looking for a *specific past run's*
output, check subdirectories under `vibe/data/` rather than assuming
top-level files — the top level reflects whatever run is currently
in progress.

## Stitching segments

Use ffmpeg with a file list (the `concat:` protocol only reads the first
file for WAV):

The generated filename includes the full segment name (e.g.
`augment_seg01_generated.wav`). Build the list from whatever prefix
matches your segments:

```bash
cd vibe/data
printf "file '%s'\n" augment_seg01_generated.wav augment_seg02_generated.wav ... > concat_list.txt
ffmpeg -y -f concat -safe 0 -i concat_list.txt -acodec copy stitched.wav
```

## Seed discovery workflow

Run a few segments without `--seed` to collect random seeds, then pick a
voice by listening:

```bash
for i in 07 08 09 10 11; do
  cargo run -- synthesize <pod_id> --gpu-price <price> segment authorship_seg${i}
done
```

Check `data/runs.jsonl` for the seed used in each segment. Preferred seed
for Sarah's voice is **71463** — see [voices.md](voices.md).

## GPU requirements

**Minimum 24GB VRAM.** The 16GB RTX A4000 produces:
- Slower inference (RTF 0.45–0.87x vs 0.29–0.40x on 24GB cards)
- Hallucinations on repeated/similar phrases

`new-pod` enforces this automatically. See [quirks.md](quirks.md) for
full GPU performance data.

## VibeVoice details

- Repo: https://github.com/vibevoice-community/VibeVoice
- Commit pinned in Dockerfile: `07cb79feadd2d3fd7f47530d4c964a12857936a0`
- Reference voice: `voices/sarah/ref.wav` (copied into image as
  `en-Sarah_woman.wav`)
- cfg_scale: 1.3 (default in CLI; 2.0 introduced artifacts on longer
  segments)
- Models pre-baked in image: `vibevoice/VibeVoice-1.5B` and
  `Qwen/Qwen2.5-1.5B` (each in their own Docker layer for cache efficiency)
