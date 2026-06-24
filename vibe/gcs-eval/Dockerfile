# Build + run the gcs-eval de-risk binary as a Cloud Run Job.
# Built via Cloud Build from the gcs-eval/ dir:
#   gcloud builds submit --tag gcr.io/<project>/gcs-eval vibe/gcs-eval
# Pin to bookworm so the builder's glibc matches the distroless/cc-debian12
# runtime base (both glibc 2.36). rust:1-slim tracks trixie (glibc 2.38) and
# produces a binary the runtime can't load.
FROM rust:1-slim-bookworm AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock* ./
COPY src ./src
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
RUN cargo build --release

# distroless/cc bundles glibc + ca-certificates (no apt needed for a Rust
# binary that makes outbound TLS calls).
FROM gcr.io/distroless/cc-debian12
COPY --from=builder /build/target/release/gcs-eval /usr/local/bin/gcs-eval
ENTRYPOINT ["/usr/local/bin/gcs-eval"]
