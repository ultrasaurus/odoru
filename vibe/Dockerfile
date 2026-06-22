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
RUN pip install --no-cache-dir hf_transfer

ENV HF_HUB_ENABLE_HF_TRANSFER=1
ENV HF_HUB_DISABLE_XET=1
RUN for i in 1 2 3; do \
      python3 -c "from huggingface_hub import snapshot_download; snapshot_download('vibevoice/VibeVoice-1.5B')" && break || sleep 5; \
    done
RUN for i in 1 2 3; do \
      python3 -c "from huggingface_hub import snapshot_download; snapshot_download('Qwen/Qwen2.5-1.5B')" && break || sleep 5; \
    done

# sshd, so RunPod can expose a direct ssh/scp port into the container
RUN apt-get update && apt-get install -y --no-install-recommends openssh-server && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /run/sshd

COPY --from=builder /build/vibe/target/release/vibe-service /usr/local/bin/vibe-service

COPY vibe/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

EXPOSE 22 3000
ENTRYPOINT ["/entrypoint.sh"]
