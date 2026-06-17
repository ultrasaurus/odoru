# syntax=docker/dockerfile:1
#
# Build from the odoru repo root:
#   docker build --platform=linux/amd64 -f vibe/Dockerfile -t vibe:latest .
#   docker tag vibe:latest dockersaura/vibe:<vN>
#   docker push dockersaura/vibe:<vN>
# Bump the version tag each time — RunPod won't pull an updated image otherwise.

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

# sshd, so RunPod can expose a direct ssh/scp port into the container
RUN apt-get update && apt-get install -y --no-install-recommends openssh-server && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /run/sshd

COPY vibe/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

EXPOSE 22
ENTRYPOINT ["/entrypoint.sh"]
