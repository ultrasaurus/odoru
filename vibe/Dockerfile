# syntax=docker/dockerfile:1
#
# Build from the odoru repo root:
#   docker build -f vibe/Dockerfile -t vibe .

FROM nvidia/cuda:12.6.3-devel-ubuntu24.04

RUN apt-get update && apt-get install -y --no-install-recommends \
        python3 python3-pip python3-venv git \
    && rm -rf /var/lib/apt/lists/*

RUN python3 -m venv /opt/venv
ENV VIRTUAL_ENV=/opt/venv
ENV PATH="/opt/venv/bin:${PATH}"

RUN pip install --no-cache-dir torch==2.4.0 --index-url https://download.pytorch.org/whl/cu124

WORKDIR /workspace
RUN git clone https://github.com/vibevoice-community/VibeVoice.git VibeVoice \
    && cd VibeVoice \
    && git checkout 07cb79feadd2d3fd7f47530d4c964a12857936a0 \
    && pip install --no-cache-dir -e .

COPY voices/sarah/ref.wav /workspace/VibeVoice/demo/voices/en-Sarah_woman.wav

# debug tools, kept as a separate late layer per project convention
RUN apt-get update && apt-get install -y --no-install-recommends procps && rm -rf /var/lib/apt/lists/*

# sshd, so RunPod can expose a direct ssh/scp port into the container
RUN apt-get update && apt-get install -y --no-install-recommends openssh-server && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /run/sshd

COPY vibe/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

EXPOSE 22
ENTRYPOINT ["/entrypoint.sh"]
