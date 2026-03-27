FROM rust:1.92-bookworm AS builder

RUN apt-get update && \
    apt-get install -y \
    cmake \
    libclang-dev \
    libssl-dev \
    pkg-config \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the workspace manifest and lockfile so image builds use the same
# dependency graph as local workspace builds.
COPY Cargo.toml Cargo.lock ./
COPY atlas ./atlas
COPY hermes-ipfs-cache ./hermes-ipfs-cache
COPY hermes-instrumentation ./hermes-instrumentation
COPY hermes-kafka ./hermes-kafka
COPY hermes-pipeline ./hermes-pipeline
COPY hermes-relay ./hermes-relay
COPY hermes-schema ./hermes-schema
COPY hermes-substream ./hermes-substream
COPY indexer_utils ./indexer_utils
COPY ipfs ./ipfs
COPY kg-indexer ./kg-indexer
COPY scoring-service ./scoring-service
COPY search-admin ./search-admin
COPY search-indexer ./search-indexer
COPY search-indexer-repository ./search-indexer-repository
COPY search-indexer-shared ./search-indexer-shared
COPY sdk ./sdk
COPY stream ./stream

RUN cargo build --release --locked -p hermes-ipfs-cache

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/hermes-ipfs-cache /usr/local/bin/hermes-ipfs-cache
COPY --from=builder /app/hermes-substream/hermes-substream-v0.1.0.spkg /hermes-substream/hermes-substream-v0.1.0.spkg

WORKDIR /
CMD ["hermes-ipfs-cache"]
