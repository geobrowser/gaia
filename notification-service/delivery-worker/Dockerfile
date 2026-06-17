FROM rust:1.92-bookworm AS builder

# Install build dependencies for protobuf (needed by hermes-instrumentation)
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
COPY hermes-instrumentation ./hermes-instrumentation
COPY notification-service/delivery-worker ./notification-service/delivery-worker

# Build from the crate directory (standalone, not as workspace member)
# Pin dependency versions to the workspace lockfile. Without it every image
# build re-resolves crates from scratch and drifts with crates.io (e.g.
# sentry-types 0.46.2 vs time 0.3.48 broke all Rust image builds, 2026-06-12).
COPY Cargo.lock ./notification-service/delivery-worker/Cargo.lock

WORKDIR /app/notification-service/delivery-worker
ENV SQLX_OFFLINE=true
RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/* && \
    useradd -r -u 1001 -g root appuser

COPY --from=builder /app/notification-service/delivery-worker/target/release/delivery-worker /usr/local/bin/delivery-worker

USER 1001

CMD ["delivery-worker"]
