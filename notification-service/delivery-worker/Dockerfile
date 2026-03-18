FROM rust:1.92-bookworm as builder

# Install build dependencies for protobuf (needed by hermes-instrumentation)
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
COPY hermes-instrumentation ./hermes-instrumentation
COPY wire ./wire
COPY notification-service/delivery-worker ./notification-service/delivery-worker

# Build from the crate directory (standalone, not as workspace member)
WORKDIR /app/notification-service/delivery-worker
ENV SQLX_OFFLINE=true
RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && \
    apt-get install -y ca-certificates libssl3 procps && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/notification-service/delivery-worker/target/release/delivery-worker /usr/local/bin/delivery-worker

CMD ["delivery-worker"]
