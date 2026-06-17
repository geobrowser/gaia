FROM rust:1.95-bookworm AS builder

RUN apt-get update && \
    apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy only required crates (no workspace Cargo.toml - build standalone)
COPY hermes-instrumentation ./hermes-instrumentation
COPY chain-tip-exporter ./chain-tip-exporter

# Pin dependency versions to the workspace lockfile. Without it every image
# build re-resolves crates from scratch and drifts with crates.io (e.g.
# sentry-types 0.46.2 vs time 0.3.48 broke all Rust image builds, 2026-06-12).
COPY Cargo.lock ./chain-tip-exporter/Cargo.lock

WORKDIR /app/chain-tip-exporter
RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/chain-tip-exporter/target/release/chain-tip-exporter /usr/local/bin/chain-tip-exporter

WORKDIR /
CMD ["chain-tip-exporter"]
