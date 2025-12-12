# 0001: Kubernetes Deployment Plan

## Status

Proposed

## Context

The `hermes-ipfs-cache` service needs to be deployed to Kubernetes alongside other Hermes services. It requires:
- PostgreSQL for cache storage
- Access to IPFS gateway
- Connection to hermes-substream via substreams endpoint
- Cursor persistence for restart recovery

Current Hermes services are deployed as Jobs to the `kafka` namespace on DigitalOcean Kubernetes.

## Implementation Plan

### Phase 1: Dockerfile

Create `hermes-ipfs-cache/Dockerfile`:

```dockerfile
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

# Copy workspace dependencies
COPY Cargo.toml Cargo.lock ./
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
```

### Phase 2: Kubernetes Manifests

#### 2.1 Database Credentials Secret

Create secret for PostgreSQL connection (manual step, not in git):

```bash
kubectl create secret generic ipfs-cache-db-credentials \
  --namespace=kafka \
  --from-literal=DATABASE_URL="postgres://user:pass@host:5432/dbname?sslmode=require"
```

#### 2.2 Substreams Credentials Secret

Create secret for substreams API token (manual step, not in git):

```bash
kubectl create secret generic substreams-credentials \
  --namespace=kafka \
  --from-literal=SUBSTREAMS_API_TOKEN="<token>" \
  --from-literal=SUBSTREAMS_ENDPOINT="https://mainnet.eth.streamingfast.io"
```

#### 2.3 Deployment Manifest

Create `hermes/k8s/hermes-ipfs-cache.yaml`:

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: hermes-ipfs-cache
  namespace: kafka
spec:
  replicas: 1
  selector:
    matchLabels:
      app: hermes-ipfs-cache
  template:
    metadata:
      labels:
        app: hermes-ipfs-cache
    spec:
      imagePullSecrets:
        - name: regcred
      containers:
        - name: hermes-ipfs-cache
          image: registry.digitalocean.com/geo/hermes-ipfs-cache:latest
          imagePullPolicy: Always
          env:
            - name: DATABASE_URL
              valueFrom:
                secretKeyRef:
                  name: ipfs-cache-db-credentials
                  key: DATABASE_URL
            - name: SUBSTREAMS_ENDPOINT
              valueFrom:
                secretKeyRef:
                  name: substreams-credentials
                  key: SUBSTREAMS_ENDPOINT
            - name: SUBSTREAMS_API_TOKEN
              valueFrom:
                secretKeyRef:
                  name: substreams-credentials
                  key: SUBSTREAMS_API_TOKEN
            - name: IPFS_GATEWAY
              value: "https://gateway.ipfs.io/ipfs/"
            - name: START_BLOCK
              value: "0"
            - name: RUST_LOG
              value: "info,hermes_ipfs_cache=debug"
          resources:
            requests:
              memory: "256Mi"
              cpu: "250m"
            limits:
              memory: "1Gi"
              cpu: "1000m"
```

**Note**: Using Deployment instead of Job because:
- Service runs continuously (streams forever with `END_BLOCK=0`)
- Needs automatic restart on failure
- Cursor persistence allows resuming from last position

#### 2.4 Update Kustomization

Add to `hermes/k8s/kustomization.yaml`:

```yaml
resources:
  - namespace.yaml
  - hermes-processor.yaml
  - hermes-ipfs-cache.yaml  # Add this
  - atlas.yaml
  - kafka-ui.yaml
```

### Phase 3: Database Setup

#### 3.1 Option A: DigitalOcean Managed PostgreSQL

Use existing or create new managed database:

```bash
# Create database cluster (if needed)
doctl databases create ipfs-cache \
  --engine pg \
  --region nyc1 \
  --size db-s-1vcpu-1gb \
  --num-nodes 1

# Get connection string
doctl databases connection <db-id> --format URI
```

#### 3.2 Option B: In-Cluster PostgreSQL

Deploy PostgreSQL to the cluster (for dev/staging):

```yaml
apiVersion: apps/v1
kind: StatefulSet
metadata:
  name: ipfs-cache-postgres
  namespace: kafka
spec:
  serviceName: ipfs-cache-postgres
  replicas: 1
  selector:
    matchLabels:
      app: ipfs-cache-postgres
  template:
    metadata:
      labels:
        app: ipfs-cache-postgres
    spec:
      containers:
        - name: postgres
          image: postgres:16
          env:
            - name: POSTGRES_DB
              value: ipfs_cache
            - name: POSTGRES_USER
              valueFrom:
                secretKeyRef:
                  name: ipfs-cache-db-credentials
                  key: POSTGRES_USER
            - name: POSTGRES_PASSWORD
              valueFrom:
                secretKeyRef:
                  name: ipfs-cache-db-credentials
                  key: POSTGRES_PASSWORD
          ports:
            - containerPort: 5432
          volumeMounts:
            - name: data
              mountPath: /var/lib/postgresql/data
  volumeClaimTemplates:
    - metadata:
        name: data
      spec:
        accessModes: ["ReadWriteOnce"]
        resources:
          requests:
            storage: 10Gi
---
apiVersion: v1
kind: Service
metadata:
  name: ipfs-cache-postgres
  namespace: kafka
spec:
  selector:
    app: ipfs-cache-postgres
  ports:
    - port: 5432
```

#### 3.3 Schema Migration

Run schema migration before first deployment:

```bash
# Port forward to database
kubectl port-forward -n kafka svc/ipfs-cache-postgres 5432:5432

# Run migrations
psql postgres://user:pass@localhost:5432/ipfs_cache <<EOF
CREATE TABLE IF NOT EXISTS ipfs_cache (
    uri TEXT PRIMARY KEY,
    json JSONB,
    block TEXT NOT NULL,
    space_id TEXT NOT NULL,
    is_errored BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE TABLE IF NOT EXISTS meta (
    id TEXT PRIMARY KEY,
    cursor TEXT NOT NULL,
    block_number TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_ipfs_cache_space_id ON ipfs_cache(space_id);
CREATE INDEX IF NOT EXISTS idx_ipfs_cache_block ON ipfs_cache(block);
EOF
```

### Phase 4: CI/CD

#### 4.1 GitHub Actions Workflow

Add to `.github/workflows/` a workflow for building and pushing the image:

```yaml
name: Build hermes-ipfs-cache

on:
  push:
    branches: [main]
    paths:
      - 'hermes-ipfs-cache/**'
      - 'hermes-relay/**'
      - 'hermes-substream/**'
      - 'stream/**'
      - 'wire/**'
      - 'ipfs/**'

jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      
      - name: Install doctl
        uses: digitalocean/action-doctl@v2
        with:
          token: ${{ secrets.DIGITALOCEAN_ACCESS_TOKEN }}
      
      - name: Log in to DO Container Registry
        run: doctl registry login --expiry-seconds 1200
      
      - name: Build and push
        run: |
          docker build -t registry.digitalocean.com/geo/hermes-ipfs-cache:latest \
            -f hermes-ipfs-cache/Dockerfile .
          docker push registry.digitalocean.com/geo/hermes-ipfs-cache:latest
      
      - name: Deploy to Kubernetes
        run: |
          doctl kubernetes cluster kubeconfig save <cluster-name>
          kubectl rollout restart deployment/hermes-ipfs-cache -n kafka
```

### Phase 5: Local Development

#### 5.1 Add to docker-compose.yaml

```yaml
services:
  # ... existing services ...

  ipfs-cache-postgres:
    image: postgres:16
    environment:
      POSTGRES_DB: ipfs_cache
      POSTGRES_USER: postgres
      POSTGRES_PASSWORD: postgres
    ports:
      - "5433:5432"
    volumes:
      - ipfs-cache-data:/var/lib/postgresql/data

  hermes-ipfs-cache:
    build:
      context: ..
      dockerfile: hermes-ipfs-cache/Dockerfile
    environment:
      DATABASE_URL: postgres://postgres:postgres@ipfs-cache-postgres:5432/ipfs_cache
      IPFS_GATEWAY: https://gateway.ipfs.io/ipfs/
      SUBSTREAMS_ENDPOINT: ${SUBSTREAMS_ENDPOINT}
      SUBSTREAMS_API_TOKEN: ${SUBSTREAMS_API_TOKEN}
      RUST_LOG: info,hermes_ipfs_cache=debug
    depends_on:
      - ipfs-cache-postgres

volumes:
  kafka-data:
  ipfs-cache-data:
```

#### 5.2 Running Locally

```bash
# With docker-compose
cd hermes
docker-compose up ipfs-cache-postgres hermes-ipfs-cache

# Or run service directly
DATABASE_URL=postgres://postgres:postgres@localhost:5433/ipfs_cache \
IPFS_GATEWAY=https://gateway.ipfs.io/ipfs/ \
SUBSTREAMS_ENDPOINT=https://... \
SUBSTREAMS_API_TOKEN=... \
cargo run -p hermes-ipfs-cache
```

## File Changes Summary

| File | Action | Description |
|------|--------|-------------|
| `hermes-ipfs-cache/Dockerfile` | Create | Multi-stage build |
| `hermes/k8s/hermes-ipfs-cache.yaml` | Create | Deployment manifest |
| `hermes/k8s/kustomization.yaml` | Modify | Add new resource |
| `hermes/docker-compose.yaml` | Modify | Add local dev services |
| `.github/workflows/hermes-ipfs-cache.yaml` | Create | CI/CD pipeline |

## Secrets Required

| Secret | Keys | Description |
|--------|------|-------------|
| `ipfs-cache-db-credentials` | `DATABASE_URL` | PostgreSQL connection string |
| `substreams-credentials` | `SUBSTREAMS_ENDPOINT`, `SUBSTREAMS_API_TOKEN` | Substreams access |
| `regcred` | (existing) | Container registry credentials |

## Monitoring

### Health Checks

Consider adding a health endpoint or using cursor progress as liveness indicator:

```bash
# Check cursor progress
kubectl exec -n kafka deploy/hermes-ipfs-cache -- \
  psql $DATABASE_URL -c "SELECT * FROM meta WHERE id = 'hermes_ipfs_cache';"
```

### Logs

```bash
kubectl logs -n kafka -l app=hermes-ipfs-cache --tail=100 -f
```

## Future Improvements

1. **Health endpoint**: HTTP endpoint for k8s liveness/readiness probes
2. **Metrics**: Prometheus metrics for fetch latency, cache hit rate, etc.
3. **Horizontal scaling**: Multiple replicas with partition-based sharding
4. **Connection pooling**: PgBouncer for database connection management
