# k6 with xk6-kafka extension for load testing
# Build stage: compile k6 with kafka extension
FROM golang:alpine AS builder

# Install git and build dependencies
RUN apk add --no-cache git build-base

# Set GOTOOLCHAIN to auto-download newer Go if required by dependencies
ENV GOTOOLCHAIN=auto

# Install xk6
RUN go install go.k6.io/xk6/cmd/xk6@latest

# Build k6 with kafka extension
RUN xk6 build \
    --with github.com/mostafa/xk6-kafka@latest \
    --output /k6

# Runtime stage: minimal image with k6 binary
FROM alpine:3.19

# Install ca-certificates for HTTPS connections
RUN apk add --no-cache ca-certificates

# Copy k6 binary from builder
COPY --from=builder /k6 /usr/bin/k6

# Create app directory
WORKDIR /scripts

# Copy test scripts
COPY . /scripts/

# Default entrypoint
ENTRYPOINT ["k6"]

# Default command (can be overridden)
CMD ["--help"]

