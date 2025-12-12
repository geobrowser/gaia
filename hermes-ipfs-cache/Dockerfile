FROM rust:1.92-bookworm as builder

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
COPY hermes-ipfs-cache ./hermes-ipfs-cache
COPY hermes-relay ./hermes-relay
COPY hermes-substream ./hermes-substream
COPY stream ./stream
COPY wire ./wire
COPY ipfs ./ipfs

# Build from the crate directory (standalone, not as workspace member)
WORKDIR /app/hermes-ipfs-cache
RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/hermes-ipfs-cache/target/release/hermes-ipfs-cache /usr/local/bin/hermes-ipfs-cache

CMD ["hermes-ipfs-cache"]
