FROM rust:1.92-bookworm as builder

RUN apt-get update && \
    apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy only required crates (no workspace Cargo.toml - build standalone)
COPY hermes-instrumentation ./hermes-instrumentation
COPY chain-tip-exporter ./chain-tip-exporter

WORKDIR /app/chain-tip-exporter
RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/chain-tip-exporter/target/release/chain-tip-exporter /usr/local/bin/chain-tip-exporter

WORKDIR /
CMD ["chain-tip-exporter"]
