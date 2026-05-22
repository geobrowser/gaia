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
COPY sdk ./sdk
COPY scoring-service/vote-indexer ./scoring-service/vote-indexer

# Build from the crate directory (standalone, not as workspace member)
WORKDIR /app/scoring-service/vote-indexer
ENV SQLX_OFFLINE=true
RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/scoring-service/vote-indexer/target/release/vote-indexer /usr/local/bin/vote-indexer

CMD ["vote-indexer"]
