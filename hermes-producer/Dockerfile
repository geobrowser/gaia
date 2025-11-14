FROM --platform=linux/amd64 rust:1.85-bookworm as builder

# Install build dependencies for rdkafka
RUN apt-get update && \
    apt-get install -y \
    cmake \
    libclang-dev \
    libssl-dev \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy only what we need - no workspace complexity
COPY hermes-producer ./hermes-producer
COPY hermes-schema ./hermes-schema
COPY wire ./wire

# Build directly from the hermes-producer directory
WORKDIR /app/hermes-producer
RUN cargo build --release --target x86_64-unknown-linux-gnu

FROM --platform=linux/amd64 debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/hermes-producer/target/x86_64-unknown-linux-gnu/release/hermes-producer /usr/local/bin/hermes-producer

ENV KAFKA_BROKER=kafka-broker-0.broker.kafka.svc.cluster.local:29092

CMD ["hermes-producer"]
