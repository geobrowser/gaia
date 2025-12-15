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
COPY hermes-pipeline ./hermes-pipeline
COPY hermes-relay ./hermes-relay
COPY hermes-kafka ./hermes-kafka
COPY hermes-schema ./hermes-schema
COPY hermes-substream ./hermes-substream
COPY stream ./stream
COPY wire ./wire

# Build from the crate directory (standalone, not as workspace member)
WORKDIR /app/hermes-pipeline
RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/hermes-pipeline/target/release/hermes-pipeline /usr/local/bin/hermes-pipeline

# Default environment variables
ENV KAFKA_BROKER=kafka-broker-0.broker.kafka.svc.cluster.local:29092

CMD ["hermes-pipeline"]
