FROM rust:1.92-bookworm as builder

# Install build dependencies for rdkafka
RUN apt-get update && \
    apt-get install -y \
    cmake \
    libclang-dev \
    libssl-dev \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy only what we need
COPY hermes-processor ./hermes-processor
COPY hermes-schema ./hermes-schema
COPY mock-substream ./mock-substream
COPY wire ./wire

# Build from the hermes-processor directory
WORKDIR /app/hermes-processor
RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/hermes-processor/target/release/hermes-processor /usr/local/bin/hermes-processor

ENV KAFKA_BROKER=kafka-broker-0.broker.kafka.svc.cluster.local:29092

CMD ["hermes-processor"]
