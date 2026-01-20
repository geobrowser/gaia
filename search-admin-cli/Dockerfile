# Multi-stage build for search-admin CLI tool
FROM rust:1.83-slim-bookworm AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy workspace files
COPY Cargo.toml Cargo.lock ./

# Copy all workspace members (needed for local path dependencies)
COPY search-admin-cli ./search-admin-cli
COPY search-indexer-repository ./search-indexer-repository
COPY search-indexer-shared ./search-indexer-shared

# Build the CLI tool in release mode
RUN cargo build --release --bin search-admin

# Runtime stage
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Copy the binary from builder
COPY --from=builder /build/target/release/search-admin /usr/local/bin/search-admin

# Create non-root user
RUN useradd -m -u 1000 search-admin && \
    chown search-admin:search-admin /usr/local/bin/search-admin

USER search-admin

ENTRYPOINT ["search-admin"]
CMD ["--help"]
