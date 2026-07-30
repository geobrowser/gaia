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
COPY indexer_utils ./indexer_utils
COPY sdk ./sdk
COPY kg-indexer ./kg-indexer

# Build from the crate directory (standalone, not as workspace member)
# Pin dependency versions to the workspace lockfile. Without it every image
# build re-resolves crates from scratch and drifts with crates.io (e.g.
# sentry-types 0.46.2 vs time 0.3.48 broke all Rust image builds, 2026-06-12).
COPY Cargo.lock ./kg-indexer/Cargo.lock

WORKDIR /app/kg-indexer
ENV SQLX_OFFLINE=true
RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/kg-indexer/target/release/kg-indexer /usr/local/bin/kg-indexer
# Remediation/diagnostic binaries (#823, #824). `cargo build --release` already
# produces them, but without these COPYs they are discarded with the builder
# stage and cannot be run in-cluster — the same omission #826 hit with
# ranking-indexer's rolling_sweep and #849 with reconcile_errored.
COPY --from=builder /app/kg-indexer/target/release/audit_batch_replay /usr/local/bin/audit_batch_replay
COPY --from=builder /app/kg-indexer/target/release/check_replay_safety /usr/local/bin/check_replay_safety
COPY --from=builder /app/kg-indexer/target/release/replay_edit /usr/local/bin/replay_edit
COPY --from=builder /app/kg-indexer/target/release/cleanup_value_orphans /usr/local/bin/cleanup_value_orphans
COPY --from=builder /app/kg-indexer/target/release/fix_misattribution /usr/local/bin/fix_misattribution
COPY --from=builder /app/kg-indexer/target/release/repair_version_chains /usr/local/bin/repair_version_chains

CMD ["kg-indexer"]
