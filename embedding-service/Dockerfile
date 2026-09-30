# Debian trixie, not bookworm: the prebuilt static ONNX Runtime that `ort` links needs libstdc++
# from GCC >= 13 on both x86_64 and aarch64 (measured 2026-09-29; see
# docs/tech-designs/semantic-search.md, "Spike results"). Same toolchain pin as every other crate.
FROM rust:1.92-trixie AS builder

WORKDIR /app

# Copy only required crates (no workspace Cargo.toml - build standalone)
COPY embedding ./embedding
COPY embedding-service ./embedding-service

# Pin dependency versions to the workspace lockfile.
COPY Cargo.lock ./embedding-service/Cargo.lock

WORKDIR /app/embedding-service
RUN cargo build --release

# Bake the bundles this image serves. Weights are fetched from the source the descriptor pins and
# verified against the committed hashes; the slot id (the directory name) is derived, never typed.
RUN for spec in bundles/*/bundle.json; do \
      ./target/release/embedding-service bundle fetch --spec "$spec" --out /models || exit 1; \
    done && ls -la /models

FROM debian:trixie-slim

COPY --from=builder /app/embedding-service/target/release/embedding-service /usr/local/bin/embedding-service
COPY --from=builder /models /models

ENV EMBEDDING_MODELS_DIR=/models
ENV PORT=8080
EXPOSE 8080

CMD ["embedding-service", "serve"]
