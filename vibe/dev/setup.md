# Setup notes: pod + VibeVoice checkout

## RunPod pod

- Created via `cargo run -- new-pod gpu` (template `pqszh5ec2m`,
  network volume `f6s2dk7onh`, named `vibevoice`) or in the RunPod
  UI for easier control of pricing.
- Image: `runpod/pytorch:2.4.0-py3.11-cuda12.4.1-devel-ubuntu22.04`.
- Connect: `cargo run -- ssh <pod_id>` — prints
  `ssh -i ~/.ssh/runpod -p <port> root@<publicIp>`. The
  `<pod_id>@ssh.runpod.io` proxy form returned "Permission denied
  (publickey)" for this pod — use the direct IP+port form instead.
- `/workspace` (on the network volume, persists across
  recreate/terminate of the pod itself):
  - `VibeVoice/` — the inference checkout (see below)
  - `hf_cache/` — HuggingFace model cache
  - `output*/`, `run*.log` — generation outputs/logs from past runs
    (`output_norm` etc. correspond to the `data/*normalized*.txt`
    inputs)
- GPU availability: pod creation/start can fail with "not enough
  free GPUs on the host machine" — workaround is terminate and
  recreate from the template (`pqszh5ec2m`). Cost was ~$2/hr while
  running; terminate when not in use.

## VibeVoice checkout (`vibe/vv/`)

- `git clone https://github.com/vibevoice-community/VibeVoice.git vv`
- Commit in use: `07cb79feadd2d3fd7f47530d4c964a12857936a0`
  ("Update README.md", 2026-06-12). Not pinned via submodule/tag —
  record this here since `vv/` is gitignored from odoru and the repo
  could move on.
- `vv/demo/inference_from_file.py` is the script used to generate the
  `*_generated.wav` files from `data/*.txt` inputs (one "Speaker N:"
  line per paragraph). Takes a `--cfg-scale` argument — see
  [vibevoice.md](vibevoice.md) for how this affects output quality
  (2.0 fixes silence/crowd-noise for short sections but introduces
  artifacts over longer full-file runs).
- `vv/demo/voices/` has sample stock voice wavs (en-Alice_woman,
  en-Frank_man, etc.), but the runs so far use a custom reference
  voice instead — placed in that same directory and renamed to match
  the stock `en-<Name>_<gender>.wav` pattern (`en-Sarah_woman.wav`),
  since `inference_from_file.py` appears to expect that naming to pick
  up a custom voice. Earlier notes referred to this file as
  `voices/sarah/ref.wav`; confirm the actual path/name on `/workspace`
  before the next run.

## Open TODO: pip installs don't survive pod recreation

`pip install -e /workspace/VibeVoice` installs into the container's
site-packages, not `/workspace` (the network volume) — so every time the
pod is recreated, `vibevoice` has to be reinstalled (~30s) before
inference will run (`ModuleNotFoundError: No module named 'vibevoice'`).
Consider creating a venv on `/workspace` or using `pip install
--target=/workspace/...` so the install persists across recreates.

## Local tooling (`vibe/`)

- Standalone Rust CLI/workspace, see [README.md](README.md) for
  commands (`new-pod`, `ssh`, `download`, `silencedetect`,
  `normalize`, etc.).
- `.env` (gitignored, see `.env.example`): `RUNPOD_API_KEY`,
  `NETWORK_VOLUME_ID`, `$TEMPLATE`.
