# syntax=docker/dockerfile:1

FROM python:3.12-trixie AS chef
# python3-dev needed for pyo3 (used by tts and dl crates)
RUN apt-get update && apt-get install -y --no-install-recommends \
    python3-dev g++ curl \
    && rm -rf /var/lib/apt/lists/*
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain 1.95
ENV PATH="/root/.cargo/bin:${PATH}"
RUN cargo install cargo-chef
WORKDIR /src

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY app/ app/
COPY config/ config/
COPY cli/ cli/
COPY dl/ dl/
COPY py-venv/ py-venv/
COPY tts/ tts/
COPY util/ util/
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /src/recipe.json recipe.json
# ort (tts crate) is CPU-only for now; CUDA build is a future iteration
RUN cargo chef cook --recipe-path recipe.json
COPY Cargo.toml Cargo.lock ./
COPY app/ app/
COPY config/ config/
COPY cli/ cli/
COPY dl/ dl/
COPY py-venv/ py-venv/
COPY tts/ tts/
COPY util/ util/
# debug build for now -- testing deploy mechanics, will use hosted server for dev
RUN cargo build -p app

FROM python:3.12-slim-trixie AS runtime

RUN python3 -m venv /opt/venv \
    && /opt/venv/bin/pip install --no-cache-dir "misaki[en]" click trafilatura soundfile \
    && /opt/venv/bin/python -m spacy download en_core_web_sm

# keep this very late, esp after spacy, so its quicker to download
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates && rm -rf /var/lib/apt/lists/*

# debug tools, kept as a separate late layer per project convention
RUN apt-get update && apt-get install -y --no-install-recommends procps && rm -rf /var/lib/apt/lists/*

ENV VIRTUAL_ENV=/opt/venv
ENV PATH="/opt/venv/bin:${PATH}"

COPY --from=builder /src/target/debug/server /usr/local/bin/server
EXPOSE 3000
CMD ["server"]
