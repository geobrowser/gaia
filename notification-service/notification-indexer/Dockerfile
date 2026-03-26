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
COPY wire ./wire
COPY notification-service/notification-indexer ./notification-service/notification-indexer

# Build from the crate directory (standalone, not as workspace member)
WORKDIR /app/notification-service/notification-indexer
ENV SQLX_OFFLINE=true
RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/* && \
    useradd -r -u 1001 -g root appuser

COPY --from=builder /app/notification-service/notification-indexer/target/release/notification-indexer /usr/local/bin/notification-indexer

USER 1001

CMD ["notification-indexer"]
