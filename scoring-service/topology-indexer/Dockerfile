FROM rust:1.92-bookworm as builder

# Install build dependencies for rdkafka and protobuf
RUN apt-get update && \
    apt-get install -y \
    cmake \
    libclang-dev \
    libssl-dev \
    pkg-config \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy only required crates (no workspace Cargo.toml - build standalone)
COPY hermes-schema ./hermes-schema
COPY hermes-instrumentation ./hermes-instrumentation
COPY hermes-kafka ./hermes-kafka
COPY scoring-service/topology-indexer ./scoring-service/topology-indexer

# Build from the crate directory (standalone, not as workspace member)
# Pin dependency versions to the workspace lockfile. Without it every image
# build re-resolves crates from scratch and drifts with crates.io (e.g.
# sentry-types 0.46.2 vs time broke all Rust image builds, 2026-06-12).
COPY Cargo.lock ./scoring-service/topology-indexer/Cargo.lock

WORKDIR /app/scoring-service/topology-indexer
ENV SQLX_OFFLINE=true
RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/scoring-service/topology-indexer/target/release/topology-indexer /usr/local/bin/topology-indexer

CMD ["topology-indexer"]
