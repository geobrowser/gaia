# ADR-001: Kubernetes Deployment for Geo API

## Status

Proposed

## Date

2025-12-18

## Context

The Geo API is a Bun/TypeScript application that provides:
- GraphQL API (PostGraphile) for the knowledge graph
- Space deployment endpoints (Personal and Public DAOs)
- IPFS upload services
- Edit calldata generation

Currently, the API has no containerization or Kubernetes deployment configuration. Other services in the monorepo (hermes-pipeline, atlas, search-indexer, scoring-service) are already deployed to our DigitalOcean Kubernetes cluster using a consistent pattern.

### Current Infrastructure

- **Kubernetes Cluster**: DigitalOcean Kubernetes (DOKS)
- **Container Registry**: `registry.digitalocean.com/geo`
- **CI/CD**: GitHub Actions with path-based triggers
- **Secrets Management**: Template files + manual `kubectl create secret`
- **Deployment Method**: Direct `kubectl apply` (ArgoCD for visualization only)

### API Technology Stack

- **Runtime**: Bun
- **Web Framework**: Hono
- **Database**: PostgreSQL via Drizzle ORM
- **GraphQL**: PostGraphile + GraphQL Yoga
- **Blockchain**: viem/ethers for Ethereum interactions
- **Telemetry**: OpenTelemetry with Axiom

## Decision

We will deploy the Geo API to Kubernetes following the established patterns in this repository.

### 1. Container Image

**Dockerfile** (`api/Dockerfile`):

```dockerfile
FROM oven/bun:1 AS base
WORKDIR /app

# Install dependencies
FROM base AS deps
COPY package.json bun.lock ./
RUN bun install --frozen-lockfile --production

# Build stage (if needed for any compilation)
FROM base AS builder
COPY --from=deps /app/node_modules ./node_modules
COPY . .

# Production image
FROM base AS runner
WORKDIR /app

ENV NODE_ENV=production

# Create non-root user
RUN addgroup --system --gid 1001 nodejs && \
    adduser --system --uid 1001 geo
USER geo

COPY --from=builder --chown=geo:nodejs /app/node_modules ./node_modules
COPY --from=builder --chown=geo:nodejs /app/src ./src
COPY --from=builder --chown=geo:nodejs /app/drizzle ./drizzle
COPY --from=builder --chown=geo:nodejs /app/package.json ./
COPY --from=builder --chown=geo:nodejs /app/main.ts ./
COPY --from=builder --chown=geo:nodejs /app/drizzle.config.ts ./
COPY --from=builder --chown=geo:nodejs /app/tsconfig.json ./

EXPOSE 3000

CMD ["bun", "run", "main.ts"]
```

**Image**: `registry.digitalocean.com/geo/api:latest`

### 2. Kubernetes Namespace

Create dedicated `api` namespace for isolation (consistent with `kafka`, `search`, `scoring` namespaces).

**File**: `api/k8s/namespace.yaml`

```yaml
apiVersion: v1
kind: Namespace
metadata:
  name: api
  labels:
    app.kubernetes.io/name: api
    app.kubernetes.io/part-of: geo
```

### 3. Secrets

**Required Secrets** (create via `kubectl create secret`):

| Secret | Key | Description |
|--------|-----|-------------|
| `api-secrets` | `DATABASE_URL` | PostgreSQL connection string |
| `api-secrets` | `RPC_ENDPOINT` | Ethereum RPC URL |
| `api-secrets` | `CHAIN_ID` | `80451` (mainnet) or `19411` (testnet) |
| `api-secrets` | `DEPLOYER_PK` | Private key for DAO deployment |
| `api-secrets` | `IPFS_KEY` | Primary IPFS gateway API key |
| `api-secrets` | `IPFS_GATEWAY_WRITE` | Primary IPFS write gateway URL |
| `api-secrets` | `IPFS_ALTERNATIVE_GATEWAY_KEY` | Alternative IPFS gateway key |
| `api-secrets` | `IPFS_ALTERNATIVE_GATEWAY_WRITE` | Alternative IPFS write gateway URL |
| `api-secrets` | `TELEMETRY_TOKEN` | Axiom API token (optional) |
| `regcred` | - | Docker registry credentials |

**Template File**: `api/k8s/secrets.yaml`

```yaml
# Template - DO NOT commit actual secrets
# Create secrets manually:
#
# kubectl create secret generic api-secrets \
#   --namespace=api \
#   --from-literal=DATABASE_URL='postgresql://...' \
#   --from-literal=RPC_ENDPOINT='https://...' \
#   --from-literal=CHAIN_ID='80451' \
#   --from-literal=DEPLOYER_PK='0x...' \
#   --from-literal=IPFS_KEY='...' \
#   --from-literal=IPFS_GATEWAY_WRITE='https://...' \
#   --from-literal=IPFS_ALTERNATIVE_GATEWAY_KEY='...' \
#   --from-literal=IPFS_ALTERNATIVE_GATEWAY_WRITE='https://...' \
#   --from-literal=TELEMETRY_TOKEN='...'
#
# kubectl create secret docker-registry regcred \
#   --namespace=api \
#   --docker-server=registry.digitalocean.com \
#   --docker-username=<token> \
#   --docker-password=<token>

apiVersion: v1
kind: Secret
metadata:
  name: api-secrets
  namespace: api
type: Opaque
stringData:
  DATABASE_URL: "REPLACE_ME"
  RPC_ENDPOINT: "REPLACE_ME"
  CHAIN_ID: "REPLACE_ME"
  DEPLOYER_PK: "REPLACE_ME"
  IPFS_KEY: "REPLACE_ME"
  IPFS_GATEWAY_WRITE: "REPLACE_ME"
  IPFS_ALTERNATIVE_GATEWAY_KEY: "REPLACE_ME"
  IPFS_ALTERNATIVE_GATEWAY_WRITE: "REPLACE_ME"
  TELEMETRY_TOKEN: "REPLACE_ME"
```

### 4. Database Migrations

Database migrations are handled via an **init container** that runs `bun run db:migrate` before the main API container starts.

**Why init container?**
- Drizzle migrations are idempotent (safe to run multiple times)
- Main container won't start if migrations fail
- No changes to Dockerfile needed
- Native Kubernetes pattern

**Trade-off**: Migrations run on every pod start (including restarts and scaling). This is acceptable for now since Drizzle only applies pending migrations. Future optimization: use a Kubernetes Job triggered only on schema changes.

### 5. Deployment and Service

**File**: `api/k8s/api.yaml`

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: api
  namespace: api
  labels:
    app: api
spec:
  replicas: 2
  selector:
    matchLabels:
      app: api
  template:
    metadata:
      labels:
        app: api
    spec:
      securityContext:
        runAsNonRoot: true
        runAsUser: 1001
        fsGroup: 1001
      imagePullSecrets:
        - name: regcred
      initContainers:
        - name: migrate
          image: registry.digitalocean.com/geo/api:latest
          imagePullPolicy: Always
          command: ["bun", "run", "db:migrate"]
          env:
            - name: DATABASE_URL
              valueFrom:
                secretKeyRef:
                  name: api-secrets
                  key: DATABASE_URL
          securityContext:
            allowPrivilegeEscalation: false
            capabilities:
              drop:
                - ALL
      containers:
        - name: api
          image: registry.digitalocean.com/geo/api:latest
          imagePullPolicy: Always
          ports:
            - containerPort: 3000
              protocol: TCP
          securityContext:
            allowPrivilegeEscalation: false
            capabilities:
              drop:
                - ALL
          env:
            - name: DATABASE_URL
              valueFrom:
                secretKeyRef:
                  name: api-secrets
                  key: DATABASE_URL
            - name: RPC_ENDPOINT
              valueFrom:
                secretKeyRef:
                  name: api-secrets
                  key: RPC_ENDPOINT
            - name: CHAIN_ID
              valueFrom:
                secretKeyRef:
                  name: api-secrets
                  key: CHAIN_ID
            - name: DEPLOYER_PK
              valueFrom:
                secretKeyRef:
                  name: api-secrets
                  key: DEPLOYER_PK
            - name: IPFS_KEY
              valueFrom:
                secretKeyRef:
                  name: api-secrets
                  key: IPFS_KEY
            - name: IPFS_GATEWAY_WRITE
              valueFrom:
                secretKeyRef:
                  name: api-secrets
                  key: IPFS_GATEWAY_WRITE
            - name: IPFS_ALTERNATIVE_GATEWAY_KEY
              valueFrom:
                secretKeyRef:
                  name: api-secrets
                  key: IPFS_ALTERNATIVE_GATEWAY_KEY
            - name: IPFS_ALTERNATIVE_GATEWAY_WRITE
              valueFrom:
                secretKeyRef:
                  name: api-secrets
                  key: IPFS_ALTERNATIVE_GATEWAY_WRITE
            - name: TELEMETRY_TOKEN
              valueFrom:
                secretKeyRef:
                  name: api-secrets
                  key: TELEMETRY_TOKEN
                  optional: true
          resources:
            requests:
              memory: "256Mi"
              cpu: "100m"
            limits:
              memory: "512Mi"
              cpu: "500m"
          livenessProbe:
            httpGet:
              path: /health
              port: 3000
            initialDelaySeconds: 10
            periodSeconds: 30
            timeoutSeconds: 5
          readinessProbe:
            httpGet:
              path: /health
              port: 3000
            initialDelaySeconds: 5
            periodSeconds: 10
            timeoutSeconds: 5
      restartPolicy: Always
---
apiVersion: v1
kind: Service
metadata:
  name: api
  namespace: api
  labels:
    app: api
  annotations:
    service.beta.kubernetes.io/do-loadbalancer-name: "geo-api-lb"
    service.beta.kubernetes.io/do-loadbalancer-protocol: "http"
spec:
  type: LoadBalancer
  ports:
    - name: http
      port: 80
      targetPort: 3000
      protocol: TCP
  selector:
    app: api
```

After deployment, get the external IP:
```bash
kubectl get svc api -n api
```

### 6. GitHub Actions Workflow

**File**: `.github/workflows/api-deploy.yml`

```yaml
name: Deploy API

on:
  push:
    branches:
      - main
    paths:
      - 'api/**'
      - '.github/workflows/api-deploy.yml'

env:
  REGISTRY: registry.digitalocean.com/geo
  IMAGE_NAME: api

jobs:
  build-and-deploy:
    runs-on: ubuntu-latest
    steps:
      - name: Checkout code
        uses: actions/checkout@v4

      - name: Install doctl
        uses: digitalocean/action-doctl@v2
        with:
          token: ${{ secrets.DIGITALOCEAN_ACCESS_TOKEN }}

      - name: Log in to DigitalOcean Container Registry
        run: doctl registry login --expiry-seconds 1200

      - name: Build and push Docker image
        run: |
          docker build -t ${{ env.REGISTRY }}/${{ env.IMAGE_NAME }}:${{ github.sha }} \
                       -t ${{ env.REGISTRY }}/${{ env.IMAGE_NAME }}:latest \
                       -f api/Dockerfile ./api
          docker push ${{ env.REGISTRY }}/${{ env.IMAGE_NAME }}:${{ github.sha }}
          docker push ${{ env.REGISTRY }}/${{ env.IMAGE_NAME }}:latest

      - name: Set up kubectl
        uses: azure/setup-kubectl@v4

      - name: Configure kubectl for DigitalOcean
        run: |
          doctl kubernetes cluster kubeconfig save ${{ secrets.DIGITALOCEAN_CLUSTER_NAME }}

      - name: Apply Kubernetes manifests
        run: |
          kubectl apply -f api/k8s/namespace.yaml
          kubectl apply -f api/k8s/api.yaml

      - name: Restart deployment
        run: |
          kubectl rollout restart deployment/api -n api

      - name: Wait for deployment
        run: |
          kubectl rollout status deployment/api -n api --timeout=300s

      - name: Show deployment status
        run: |
          kubectl get pods -n api
          kubectl get svc -n api
```

### 7. File Structure

```
api/
├── Dockerfile
├── k8s/
│   ├── namespace.yaml
│   ├── api.yaml
│   └── secrets.yaml          # Template only
├── docs/
│   └── plans/
│       └── 001-kubernetes-deployment.md
└── ...existing files...

.github/workflows/
└── api-deploy.yml
```

## Implementation Steps

1. **Create Dockerfile** (`api/Dockerfile`)
2. **Create k8s directory** (`api/k8s/`)
3. **Create namespace manifest** (`api/k8s/namespace.yaml`)
4. **Create secrets template** (`api/k8s/secrets.yaml`)
5. **Create deployment manifest** (`api/k8s/api.yaml`)
6. **Create GitHub Actions workflow** (`.github/workflows/api-deploy.yml`)
7. **Manually create secrets in cluster**:
   ```bash
   # Create namespace first
   kubectl apply -f api/k8s/namespace.yaml
   
   # Create secrets
   kubectl create secret generic api-secrets \
     --namespace=api \
     --from-literal=DATABASE_URL='...' \
     --from-literal=RPC_ENDPOINT='...' \
     --from-literal=CHAIN_ID='80451' \
     --from-literal=DEPLOYER_PK='...' \
     --from-literal=IPFS_KEY='...' \
     --from-literal=IPFS_GATEWAY_WRITE='...' \
     --from-literal=IPFS_ALTERNATIVE_GATEWAY_KEY='...' \
     --from-literal=IPFS_ALTERNATIVE_GATEWAY_WRITE='...' \
     --from-literal=TELEMETRY_TOKEN='...'
   
   # Create registry credentials
   kubectl create secret docker-registry regcred \
     --namespace=api \
     --docker-server=registry.digitalocean.com \
     --docker-username=<do-token> \
     --docker-password=<do-token>
   ```
8. **Merge to main** to trigger deployment

## Consequences

### Positive

- Consistent with existing deployment patterns in the repository
- Automated deployments on merge to main
- Health checks ensure service availability
- Security context follows best practices (non-root, dropped capabilities)
- Resource limits prevent runaway resource consumption
- Multiple replicas provide high availability

### Negative

- Manual secret creation required (not GitOps for secrets)
- Migrations run on every pod start (acceptable for now, can optimize later)
- LoadBalancer costs ~$12/month (acceptable for single external service)

### Risks

- `DEPLOYER_PK` contains a private key - ensure proper secret handling
- Database connection pooling may need tuning for k8s environment
- IPFS gateway dependencies may cause deployment issues if unavailable

## Future Considerations

1. **Ingress Controller**: Add nginx-ingress or similar for external HTTPS access
2. **Migration Optimization**: Use a Kubernetes Job for migrations, triggered only on schema changes in CI
3. **Horizontal Pod Autoscaler**: Scale based on CPU/memory usage
4. **PodDisruptionBudget**: Ensure availability during node maintenance
5. **External Secrets Operator**: Consider for GitOps-friendly secret management
6. **ArgoCD Application**: Add visualization in ArgoCD dashboard

## References

- Existing workflows: `.github/workflows/search-indexer-deploy.yml`
- Existing k8s configs: `hermes/k8s/`, `search-indexer-deploy/k8s/`
- Bun Docker documentation: https://bun.sh/guides/ecosystem/docker
