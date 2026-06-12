FROM rust:1.92-bookworm as builder

# Install build dependencies for rdkafka
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
COPY atlas ./atlas
COPY hermes-instrumentation ./hermes-instrumentation
COPY hermes-kafka ./hermes-kafka
COPY hermes-relay ./hermes-relay
COPY hermes-schema ./hermes-schema
COPY hermes-substream ./hermes-substream
COPY stream ./stream

# Build from the crate directory (standalone, not as workspace member)
# Pin dependency versions to the workspace lockfile. Without it every image
# build re-resolves crates from scratch and drifts with crates.io (e.g.
# sentry-types 0.46.2 vs time broke all Rust image builds, 2026-06-12).
COPY Cargo.lock ./atlas/Cargo.lock

WORKDIR /app/atlas
RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/atlas/target/release/atlas /usr/local/bin/atlas
COPY --from=builder /app/hermes-substream/hermes-substream-v0.1.0.spkg /hermes-substream/hermes-substream-v0.1.0.spkg

ENV KAFKA_BROKER=kafka-broker-0.broker.kafka.svc.cluster.local:29092
ENV KAFKA_TOPIC=topology.canonical

WORKDIR /
CMD ["atlas"]
