# syntax=docker/dockerfile:1

FROM python:3.12-trixie AS chef
RUN apt-get update && apt-get install -y --no-install-recommends \
    python3-dev g++ curl \
    && rm -rf /var/lib/apt/lists/*
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain 1.95
ENV PATH="/root/.cargo/bin:${PATH}"
RUN cargo install cargo-chef
WORKDIR /src

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY crates/lifecycle/Cargo.toml crates/lifecycle/Cargo.toml
COPY crates/server/Cargo.toml crates/server/Cargo.toml
COPY crates/pilot-worker/Cargo.toml crates/pilot-worker/Cargo.toml
COPY crates/pilot-worker/src/ crates/pilot-worker/src/
COPY crates/lifecycle/src/ crates/lifecycle/src/
COPY crates/server/src/ crates/server/src/
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /src/recipe.json recipe.json
# RUN cargo chef cook --release --recipe-path recipe.json
RUN cargo chef cook --recipe-path recipe.json
COPY Cargo.toml Cargo.lock ./
COPY crates/lifecycle/Cargo.toml crates/lifecycle/Cargo.toml
COPY crates/server/Cargo.toml crates/server/Cargo.toml
COPY crates/pilot-worker/Cargo.toml crates/pilot-worker/Cargo.toml
COPY crates/pilot-worker/src/ crates/pilot-worker/src/
COPY crates/lifecycle/src/ crates/lifecycle/src/
COPY crates/server/src/ crates/server/src/
# RUN cargo build -p pod-server --release
RUN cargo build -p pod-server

FROM python:3.12-slim-trixie AS runtime

RUN python3 -m venv /opt/venv \
    && /opt/venv/bin/pip install --no-cache-dir "misaki[en]" click trafilatura soundfile \
    && /opt/venv/bin/python -m spacy download en_core_web_sm

# keep this very late, esp after spacy, so its quicker to download    
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates procps && rm -rf /var/lib/apt/lists/*

ENV VIRTUAL_ENV=/opt/venv
ENV PATH="/opt/venv/bin:${PATH}"

# COPY --from=builder /src/target/release/pod-server /usr/local/bin/pod-server
COPY --from=builder /src/target/debug/pod-server /usr/local/bin/pod-server
EXPOSE 3000
CMD ["pod-server"]
