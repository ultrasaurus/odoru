# syntax=docker/dockerfile:1
#
# Build from the odoru repo root:
#   docker build --platform=linux/amd64 -f vibe/Dockerfile -t vibe:latest .
#   docker tag vibe:latest dockersaura/vibe:<vN>
#   docker push dockersaura/vibe:<vN>
# Bump the version tag each time — RunPod won't pull an updated image otherwise.

# ── Stage 1: cargo-chef base ─────────────────────────────────────────────
# Built on the same CUDA devel image as the runtime stage — forced-alignment's
# cudarc dependency needs nvcc, and tokenizers' esaxx-rs needs a C++ compiler;
# rust:slim-bullseye has neither and lacks the CUDA toolkit entirely.
FROM  --platform=linux/amd64 runpod/pytorch:2.4.0-py3.11-cuda12.4.1-devel-ubuntu22.04 AS chef

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    pkg-config \
    libssl-dev \
    curl \
    && rm -rf /var/lib/apt/lists/*

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"
RUN cargo install cargo-chef --locked

WORKDIR /build

# ── Stage 2: dependency recipe ──────────────────────────────────────────
# Computes a recipe.json from Cargo.toml/Cargo.lock only, so the cook step
# below can cache dependency builds without being invalidated by source edits.
FROM chef AS planner
COPY . .
# vibe/service isn't a member of the root workspace — vibe/Cargo.toml is its
# own workspace (members = [".", "service"]) — so cargo chef must run from
# inside it rather than being pointed at it via --manifest-path (unsupported).
RUN cd vibe/service && cargo chef prepare --recipe-path recipe.json

# ── Stage 3: compile vibe-service ───────────────────────────────────────
FROM chef AS builder

# No GPU is present at `docker build` time, so candle-kernels can't probe
# nvidia-smi for compute capability. Pods are RTX A4000/A6000/3090/A40 — all
# Ampere (sm_86).
ENV CUDA_COMPUTE_CAP=86

COPY --from=planner /build/vibe/service/recipe.json vibe/service/recipe.json
# cargo chef cook needs the workspace's manifest tree (not just recipe.json)
# to resolve the workspace before building stub crates — vibe's workspace has
# a path dependency on ../util, so that manifest is needed too. Manifests
# only, no source, so source edits don't bust this cached layer.
COPY vibe/Cargo.toml vibe/Cargo.toml
COPY vibe/Cargo.lock vibe/Cargo.lock
COPY vibe/service/Cargo.toml vibe/service/Cargo.toml
COPY util/Cargo.toml util/Cargo.toml
RUN cd vibe/service && cargo chef cook --release \
    --recipe-path recipe.json \
    --features cuda

COPY . .
RUN cargo build --release \
    --manifest-path vibe/service/Cargo.toml \
    --features vibe-service/cuda \
    --bin vibe-service

# ── Stage 2: RunPod GPU image ───────────────────────────────────────────────
# RunPod base image: PyTorch 2.4.0, CUDA 12.4.1, Ubuntu 22.04.
# Includes torch, CUDA drivers, and Python — no need to install them separately.
# Supports sm_50–sm_90 (all pre-Blackwell GPUs); requires driver ≥ 525.85.
FROM  --platform=linux/amd64 runpod/pytorch:2.4.0-py3.11-cuda12.4.1-devel-ubuntu22.04

WORKDIR /workspace
RUN git clone https://github.com/vibevoice-community/VibeVoice.git VibeVoice \
    && cd VibeVoice \
    && git checkout 07cb79feadd2d3fd7f47530d4c964a12857936a0

RUN pip install --no-cache-dir -e /workspace/VibeVoice

ENV HF_HOME=/workspace/.cache/huggingface
ENV HF_HUB_DISABLE_XET=1
RUN mkdir -p $HF_HOME

COPY vibe/docker/download-models.py /tmp/download-models.py
COPY vibe/docker/verify-qwen-tokenizer.py /tmp/verify-qwen-tokenizer.py
RUN python3 /tmp/download-models.py vibevoice/VibeVoice-1.5B
RUN python3 /tmp/download-models.py Qwen/Qwen2.5-1.5B
RUN python3 /tmp/download-models.py facebook/wav2vec2-base-960h
RUN python3 /tmp/verify-qwen-tokenizer.py

# sshd, so RunPod can expose a direct ssh/scp port into the container
RUN apt-get update && apt-get install -y --no-install-recommends openssh-server && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /run/sshd

COPY --from=builder /build/vibe/target/release/vibe-service /usr/local/bin/vibe-service

COPY vibe/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

# Flash Attention 2: VibeVoice's inference_from_file.py requests
# flash_attention_2 on CUDA and silently falls back to SDPA (slower, and per
# VibeVoice's own warning, less-tested / lower audio quality) if it's not
# installed. Prebuilt wheel matched to this base's torch 2.4 / cu12 / py3.11
# (pip-torch is cxx11abi=FALSE). Kept as the last RUN so it doesn't bust the
# multi-GB model-download layers above.
RUN pip install --no-cache-dir \
    "https://github.com/Dao-AILab/flash-attention/releases/download/v2.7.4.post1/flash_attn-2.7.4.post1+cu12torch2.4cxx11abiFALSE-cp311-cp311-linux_x86_64.whl"

# Override the upstream demo script with our copy, which adds --temp and
# --speed (see vibe/docker/inference_from_file.py). Kept last so editing it
# doesn't bust the heavy clone/pip/model-download layers above.
COPY vibe/docker/inference_from_file.py /workspace/VibeVoice/demo/inference_from_file.py

# Production batched-generate() entry point for POST /batches (dev/parallel.md
# "Stage 3 implementation plan"). Reads one JSON batch request from stdin,
# writes one wav per segment — see batch_inference.py's docstring.
COPY vibe/docker/batch_inference.py /workspace/VibeVoice/demo/batch_inference.py

# Standalone batched-generate() benchmark (dev/parallel.md "Max batch size is
# unknown") with its sample segments (seg41-71, copied out of vibe/data/
# since .dockerignore excludes that wholesale). Not part of the synth request
# path — trigger via POST /bench (works on Cloud Run, no shell access there)
# or run over SSH directly (`cargo run -- ssh <pod_id>`) and scp the output.
COPY vibe/docker/batch_bench.py /workspace/VibeVoice/demo/batch_bench.py
COPY vibe/docker/bench_segments/ /workspace/VibeVoice/demo/bench_segments/

# Only batch_bench.py's optional GCS upload (for the no-shell-access Cloud Run
# path) needs this — own late layer so it doesn't bust anything above.
RUN pip install --no-cache-dir google-cloud-storage

EXPOSE 22 3000
ENTRYPOINT ["/entrypoint.sh"]
