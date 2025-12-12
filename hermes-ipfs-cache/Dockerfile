FROM rust:1.85-bookworm as builder

RUN apt-get update && \
    apt-get install -y \
    cmake \
    libclang-dev \
    libssl-dev \
    pkg-config \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy workspace files
COPY Cargo.toml Cargo.lock ./

# Copy workspace dependencies
COPY hermes-ipfs-cache ./hermes-ipfs-cache
COPY hermes-relay ./hermes-relay
COPY hermes-substream ./hermes-substream
COPY stream ./stream
COPY wire ./wire
COPY ipfs ./ipfs

# Build
RUN cargo build --release -p hermes-ipfs-cache

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/hermes-ipfs-cache /usr/local/bin/hermes-ipfs-cache

CMD ["hermes-ipfs-cache"]
