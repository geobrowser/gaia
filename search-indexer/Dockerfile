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

# Copy only required crates (no workspace Cargo.toml - build standalone)
COPY search-indexer ./search-indexer
COPY search-indexer-shared ./search-indexer-shared
COPY search-indexer-repository ./search-indexer-repository
COPY hermes-schema ./hermes-schema
COPY wire ./wire
COPY indexer_utils ./indexer_utils
COPY sdk ./sdk

# Build from the crate directory (standalone, not as workspace member)
WORKDIR /app/search-indexer
RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/* && \
    # Create non-root user for running the application
    groupadd -r appuser && \
    useradd -r -g appuser -u 1000 -d /home/appuser -s /sbin/nologin appuser && \
    mkdir -p /home/appuser && \
    chown -R appuser:appuser /home/appuser

COPY --from=builder /app/search-indexer/target/release/search-indexer /usr/local/bin/search-indexer

# Default environment variables
ENV OPENSEARCH_URL=http://opensearch:9200
ENV KAFKA_BROKER=kafka:29092
ENV KAFKA_GROUP_ID=search-indexer
ENV OPENSEARCH_CONNECTION_MODE=retry

# Run as non-root user
USER appuser

CMD ["search-indexer"]

