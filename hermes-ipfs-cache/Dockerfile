FROM rust:1.92-bookworm as builder

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
COPY hermes-ipfs-cache ./hermes-ipfs-cache
COPY hermes-instrumentation ./hermes-instrumentation
COPY hermes-relay ./hermes-relay
COPY hermes-substream ./hermes-substream
COPY stream ./stream
COPY ipfs ./ipfs

# Build from the crate directory (standalone, not as workspace member)
# Pin dependency versions to the workspace lockfile. Without it every image
# build re-resolves crates from scratch and drifts with crates.io (e.g.
# sentry-types 0.46.2 vs time 0.3.48 broke all Rust image builds, 2026-06-12).
COPY Cargo.lock ./hermes-ipfs-cache/Cargo.lock

WORKDIR /app/hermes-ipfs-cache
RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/hermes-ipfs-cache/target/release/hermes-ipfs-cache /usr/local/bin/hermes-ipfs-cache
# reconcile_errored re-attempts ipfs_cache rows marked is_errored (#823). `cargo
# build --release` already produces it, but without this COPY it is discarded with
# the builder stage and the tool cannot be run in-cluster — same omission #826 hit
# with ranking-indexer's rolling_sweep.
COPY --from=builder /app/hermes-ipfs-cache/target/release/reconcile_errored /usr/local/bin/reconcile_errored
COPY --from=builder /app/hermes-substream/hermes-substream-v0.1.0.spkg /hermes-substream/hermes-substream-v0.1.0.spkg

WORKDIR /
CMD ["hermes-ipfs-cache"]
