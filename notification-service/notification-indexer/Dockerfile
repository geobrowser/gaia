FROM rust:1.92-bookworm AS builder

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
# Canonical knowledge-graph IDs (sdk crate) — path dep of notification-indexer.
COPY sdk ./sdk
COPY notification-service/notification-indexer ./notification-service/notification-indexer

# Build from the crate directory (standalone, not as workspace member)
# Pin dependency versions to the workspace lockfile. Without it every image
# build re-resolves crates from scratch and drifts with crates.io (e.g.
# sentry-types 0.46.2 vs time 0.3.48 broke all Rust image builds, 2026-06-12).
COPY Cargo.lock ./notification-service/notification-indexer/Cargo.lock

WORKDIR /app/notification-service/notification-indexer
ENV SQLX_OFFLINE=true
RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/* && \
    useradd -r -u 1001 -g root appuser

COPY --from=builder /app/notification-service/notification-indexer/target/release/notification-indexer /usr/local/bin/notification-indexer

USER 1001

CMD ["notification-indexer"]
