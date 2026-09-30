FROM rust:1.92-bookworm AS builder

WORKDIR /app

# Copy only required crates (no workspace Cargo.toml - build standalone). Standalone matters:
# `embedding` is depended on with default-features = false, and building from a workspace root
# that lists it as a member would unify its default `onnx` feature in (see search-admin/Dockerfile).
COPY embedding ./embedding
COPY search-indexer-shared ./search-indexer-shared
COPY sdk ./sdk
COPY embedding-indexer ./embedding-indexer

# Pin dependency versions to the workspace lockfile.
COPY Cargo.lock ./embedding-indexer/Cargo.lock

WORKDIR /app/embedding-indexer
RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/embedding-indexer/target/release/embedding-indexer /usr/local/bin/embedding-indexer

CMD ["embedding-indexer"]
