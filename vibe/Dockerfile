# syntax=docker/dockerfile:1
#
# Build from the odoru repo root:
#   docker build --platform=linux/amd64 -f vibe/Dockerfile -t vibe:latest .
#   docker tag vibe:latest dockersaura/vibe:<vN>
#   docker push dockersaura/vibe:<vN>
# Bump the version tag each time — RunPod won't pull an updated image otherwise.

# ── Stage 1: compile vibe-service ──────────────────────────────────────────
# GNU target (not musl) because forced-alignment links against CUDA dynamically.
FROM rust:slim-bullseye AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY . .

RUN cargo build --release \
    --manifest-path vibe/service/Cargo.toml \
    --bin vibe-service

# ── Stage 2: RunPod GPU image ───────────────────────────────────────────────
# RunPod base image: PyTorch 2.4.0, CUDA 12.4.1, Ubuntu 22.04.
# Includes torch, CUDA drivers, and Python — no need to install them separately.
# Supports sm_50–sm_90 (all pre-Blackwell GPUs); requires driver ≥ 525.85.
FROM runpod/pytorch:2.4.0-py3.11-cuda12.4.1-devel-ubuntu22.04

WORKDIR /workspace
RUN git clone https://github.com/vibevoice-community/VibeVoice.git VibeVoice \
    && cd VibeVoice \
    && git checkout 07cb79feadd2d3fd7f47530d4c964a12857936a0

RUN pip install --no-cache-dir -e /workspace/VibeVoice

COPY voices/sarah/ref.wav /workspace/VibeVoice/demo/voices/en-Sarah_woman.wav

RUN python3 -c "from huggingface_hub import snapshot_download; snapshot_download('vibevoice/VibeVoice-1.5B')"
RUN python3 -c "from huggingface_hub import snapshot_download; snapshot_download('Qwen/Qwen2.5-1.5B')"

# sshd, so RunPod can expose a direct ssh/scp port into the container
RUN apt-get update && apt-get install -y --no-install-recommends openssh-server && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /run/sshd

COPY --from=builder /build/vibe/target/release/vibe-service /usr/local/bin/vibe-service

COPY vibe/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

EXPOSE 22 3000
ENTRYPOINT ["/entrypoint.sh"]
