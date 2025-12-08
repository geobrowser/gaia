FROM rust:1.86-bookworm as builder

# Install build dependencies for rdkafka
RUN apt-get update && \
    apt-get install -y \
    cmake \
    libclang-dev \
    libssl-dev \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy dependencies
COPY atlas ./atlas
COPY hermes-schema ./hermes-schema
COPY mock-substream ./mock-substream
COPY wire ./wire

# Build from the atlas directory
WORKDIR /app/atlas
RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/atlas/target/release/atlas /usr/local/bin/atlas

ENV KAFKA_BROKER=kafka-broker-0.broker.kafka.svc.cluster.local:29092
ENV KAFKA_TOPIC=topology.canonical

CMD ["atlas"]
