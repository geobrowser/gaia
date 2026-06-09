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

COPY Cargo.toml Cargo.lock ./
COPY atlas/Cargo.toml ./atlas/Cargo.toml
COPY indexer_utils/Cargo.toml ./indexer_utils/Cargo.toml
COPY search-indexer/Cargo.toml ./search-indexer/Cargo.toml
COPY search-indexer-shared/Cargo.toml ./search-indexer-shared/Cargo.toml
COPY search-indexer-repository/Cargo.toml ./search-indexer-repository/Cargo.toml
COPY search-admin/Cargo.toml ./search-admin/Cargo.toml
COPY sdk/Cargo.toml ./sdk/Cargo.toml
COPY hermes-kafka/Cargo.toml ./hermes-kafka/Cargo.toml
COPY hermes-schema/Cargo.toml ./hermes-schema/Cargo.toml
COPY hermes-pipeline/Cargo.toml ./hermes-pipeline/Cargo.toml
COPY kg-indexer/Cargo.toml ./kg-indexer/Cargo.toml
COPY scoring-service/vote-indexer/Cargo.toml ./scoring-service/vote-indexer/Cargo.toml
COPY scoring-service/topology-indexer/Cargo.toml ./scoring-service/topology-indexer/Cargo.toml
RUN for crate in \
    atlas \
    indexer_utils \
    search-indexer \
    search-indexer-shared \
    search-indexer-repository \
    search-admin \
    sdk \
    hermes-kafka \
    hermes-schema \
    hermes-pipeline \
    kg-indexer \
    scoring-service/vote-indexer \
    scoring-service/topology-indexer; do \
    mkdir -p "$crate/src" && \
    printf 'pub fn placeholder() {}\n' > "$crate/src/lib.rs" && \
    printf 'fn main() {}\n' > "$crate/src/main.rs"; \
done
RUN mkdir -p atlas/benches hermes-pipeline/benches kg-indexer/tests && \
    printf 'fn main() {}\n' > atlas/benches/transitive.rs && \
    printf 'fn main() {}\n' > atlas/benches/canonical.rs && \
    printf 'fn main() {}\n' > atlas/benches/graph_diff.rs && \
    printf 'fn main() {}\n' > hermes-pipeline/benches/decode_bench.rs && \
    printf 'fn main() {}\n' > hermes-pipeline/benches/pipeline_bench.rs && \
    printf '#[test]\nfn placeholder() {}\n' > kg-indexer/tests/e2e.rs
COPY hermes-ipfs-cache ./hermes-ipfs-cache
COPY hermes-instrumentation ./hermes-instrumentation
COPY hermes-relay ./hermes-relay
COPY hermes-substream ./hermes-substream
COPY ipfs ./ipfs
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
