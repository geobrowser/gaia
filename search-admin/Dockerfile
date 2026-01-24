# Multi-stage build for search-admin CLI tool
FROM rust:1.92-slim-bookworm AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy only the workspace members we need
COPY search-admin ./search-admin
COPY search-indexer-repository ./search-indexer-repository
COPY search-indexer-shared ./search-indexer-shared

# Copy Cargo.lock for dependency resolution
COPY Cargo.lock ./

# Create a minimal workspace Cargo.toml with only the members we copied
RUN echo '[workspace]' > Cargo.toml && \
    echo 'members = [' >> Cargo.toml && \
    echo '    "search-admin",' >> Cargo.toml && \
    echo '    "search-indexer-repository",' >> Cargo.toml && \
    echo '    "search-indexer-shared",' >> Cargo.toml && \
    echo ']' >> Cargo.toml && \
    echo 'resolver = "2"' >> Cargo.toml

# Build the CLI tool in release mode
# Note: We copy Cargo.lock but don't use --locked because the workspace
# structure here differs from the full repo (only 3 members vs all members)
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
