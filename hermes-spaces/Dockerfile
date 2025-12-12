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

# Copy workspace configuration
COPY Cargo.toml Cargo.lock ./

# Copy all required crates
COPY hermes-spaces ./hermes-spaces
COPY hermes-relay ./hermes-relay
COPY hermes-schema ./hermes-schema
COPY stream ./stream
COPY wire ./wire

# Copy the substream package file
COPY hermes-substream/hermes-substream.spkg ./hermes-substream.spkg

# Build from the workspace root
RUN cargo build --release --package hermes-spaces

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/hermes-spaces /usr/local/bin/hermes-spaces
COPY --from=builder /app/hermes-substream.spkg /app/hermes-substream.spkg

WORKDIR /app

# Default environment variables
ENV KAFKA_BROKER=kafka-broker-0.broker.kafka.svc.cluster.local:29092

CMD ["hermes-spaces"]
