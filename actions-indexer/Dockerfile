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
COPY actions-indexer ./actions-indexer
COPY actions-indexer-pipeline ./actions-indexer-pipeline
COPY actions-indexer-repository ./actions-indexer-repository
COPY actions-indexer-shared ./actions-indexer-shared
COPY hermes-schema ./hermes-schema
COPY hermes-kafka ./hermes-kafka
COPY wire ./wire

# Copy sqlx query cache for offline compilation
COPY .sqlx ./.sqlx

# Create a minimal workspace Cargo.toml so SQLx finds the .sqlx cache at workspace root
RUN echo '[workspace]\nmembers = ["actions-indexer", "actions-indexer-pipeline", "actions-indexer-repository", "actions-indexer-shared", "hermes-schema", "hermes-kafka", "wire"]\nresolver = "2"' > Cargo.toml

# Use offline mode - queries are pre-cached in .sqlx directory
ENV SQLX_OFFLINE=true
RUN cargo build --release -p actions-indexer

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/* && \
    # Create non-root user for running the application
    groupadd -r appuser && \
    useradd -r -g appuser -u 1000 -d /home/appuser -s /sbin/nologin appuser && \
    mkdir -p /home/appuser && \
    chown -R appuser:appuser /home/appuser

COPY --from=builder /app/target/release/actions-indexer /usr/local/bin/actions-indexer

# Default environment variables
ENV DATA_SOURCE=kafka
ENV KAFKA_BROKER=kafka:29092
ENV KAFKA_CONSUMER_GROUP=actions-indexer
ENV KAFKA_TOPIC=curation.votes

# Run as non-root user
USER appuser

CMD ["actions-indexer"]

