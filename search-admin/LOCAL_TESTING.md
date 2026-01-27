# Local Testing Guide for Search Admin

This guide walks through testing the complete search-admin tool and full migration workflow using a local Kubernetes cluster with kind.

## Prerequisites

- Docker installed and running
- kubectl installed
- kind installed: `brew install kind`
- jq installed (optional, for pretty JSON output): `brew install jq`
- Rust toolchain (for local cargo testing): `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`

## Testing with Cargo (Local Development)

For rapid local development without Docker, you can test commands directly using `cargo run`:

```bash
# Set up port-forward to OpenSearch running in kind
kubectl port-forward -n search svc/opensearch 9200:9200 &
PF_PID=$!

# Set environment variables
export OPENSEARCH_URL="http://localhost:9200"
export INDEX_ALIAS="entities"

# Run commands directly with cargo
cd search-admin

# List indices
cargo run -- list-indices

# Create an index
cargo run -- create-index --version 3

# Reindex with synchronous mode
cargo run -- reindex --source-version 2 --target-version 3 --wait-for-completion

# Update alias
cargo run -- update-alias --version 3

# Delete an index (requires confirmation)
cargo run -- delete-index --version 2 --confirm --yes

# Kill port-forward when done
kill $PF_PID
```

**Note:** The `full-migration` command requires Kubernetes API access to manage deployments. For local testing of full-migration, use the Kubernetes Job approach described below.

## Quick Start

Run the automated test script:

```bash
# From the repository root
./search-indexer-deploy/tests/setup-test-environment.sh
```

Then test the migration:

```bash
# Test using the helper script
./search-indexer-deploy/tests/test-full-migration.sh 1 2

# Or test locally with cargo (see "Testing with Cargo" section below)
```

## Manual Setup Steps

### Step 1: Start kind Cluster

```bash
# Install kind if not already installed
brew install kind

# Clean up any existing test cluster (optional but recommended)
if kind get clusters 2>/dev/null | grep -q "^search-test$"; then
    echo "Deleting existing search-test cluster..."
    kind delete cluster --name search-test
fi

# Create a kind cluster
kind create cluster --name search-test

# Verify it's running
kubectl cluster-info --context kind-search-test
kubectl get nodes
```

### Step 2: Deploy OpenSearch

```bash
# Create namespace
kubectl create namespace search

# Deploy OpenSearch
kubectl apply -f - <<EOF
apiVersion: v1
kind: Service
metadata:
  name: opensearch
  namespace: search
spec:
  ports:
  - port: 9200
    targetPort: 9200
  selector:
    app: opensearch
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: opensearch
  namespace: search
spec:
  replicas: 1
  selector:
    matchLabels:
      app: opensearch
  template:
    metadata:
      labels:
        app: opensearch
    spec:
      containers:
      - name: opensearch
        image: opensearchproject/opensearch:2.11.0
        env:
        - name: discovery.type
          value: single-node
        - name: DISABLE_SECURITY_PLUGIN
          value: "true"
        - name: OPENSEARCH_JAVA_OPTS
          value: "-Xms512m -Xmx512m"
        ports:
        - containerPort: 9200
        resources:
          limits:
            memory: 1Gi
          requests:
            memory: 512Mi
EOF

# Wait for OpenSearch to be ready (may take 2-3 minutes)
# First wait for pod to be created
until kubectl get pod -l app=opensearch -n search 2>/dev/null | grep -q opensearch; do
  echo "Waiting for pod to be created..."
  sleep 2
done
# Then wait for it to be ready
kubectl wait --for=condition=ready pod -l app=opensearch -n search --timeout=300s

# Verify it's running
kubectl get pods -n search
```

### Step 3: Create Credentials Secret

```bash
kubectl create secret generic opensearch-credentials \
  --from-literal=OPENSEARCH_URL=http://opensearch.search.svc.cluster.local:9200 \
  -n search
```

### Step 4: Set Up Port Forward and Environment

```bash
# Start port-forward to OpenSearch
kubectl port-forward -n search svc/opensearch 9200:9200 &
PF_PID=$!
sleep 3

# Set environment variables for cargo
export OPENSEARCH_URL="http://localhost:9200"
export INDEX_ALIAS="entities"
```

### Step 5: Create Initial Index and Add Test Data

```bash
# Create version 1 index using cargo
cd search-admin
cargo run -- create-index --version 1

# Verify OpenSearch is accessible (port-forward should already be running)
curl -s "http://localhost:9200/_cluster/health?pretty"

# Add test documents
curl -X POST "http://localhost:9200/entities_v1/_doc/1" \
  -H "Content-Type: application/json" \
  -d '{
    "id": "0x1234567890123456789012345678901234567890",
    "name": "Ethereum Foundation",
    "entity_type": "address",
    "chain_id": 1,
    "created_at": "2024-01-01T00:00:00Z"
  }'

curl -X POST "http://localhost:9200/entities_v1/_doc/2" \
  -H "Content-Type: application/json" \
  -d '{
    "id": "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd",
    "name": "Uniswap",
    "entity_type": "address",
    "chain_id": 1,
    "created_at": "2024-01-02T00:00:00Z"
  }'

curl -X POST "http://localhost:9200/entities_v1/_doc/3" \
  -H "Content-Type: application/json" \
  -d '{
    "id": "dao:0x1234567890123456789012345678901234567890:proposal:123",
    "name": "Governance Proposal #123",
    "entity_type": "proposal",
    "chain_id": 1,
    "created_at": "2024-01-03T00:00:00Z"
  }'

# Bulk insert more documents for realistic testing
curl -X POST "http://localhost:9200/entities_v1/_bulk" \
  -H "Content-Type: application/x-ndjson" \
  -d '
{"index":{"_id":"4"}}
{"id":"0x1111111111111111111111111111111111111111","name":"Test Address 1","entity_type":"address","chain_id":1}
{"index":{"_id":"5"}}
{"id":"0x2222222222222222222222222222222222222222","name":"Test Address 2","entity_type":"address","chain_id":1}
{"index":{"_id":"6"}}
{"id":"0x3333333333333333333333333333333333333333","name":"Test Address 3","entity_type":"address","chain_id":1}
{"index":{"_id":"7"}}
{"id":"0x4444444444444444444444444444444444444444","name":"Test Address 4","entity_type":"address","chain_id":1}
{"index":{"_id":"8"}}
{"id":"0x5555555555555555555555555555555555555555","name":"Test Address 5","entity_type":"address","chain_id":1}
{"index":{"_id":"9"}}
{"id":"dao:test:proposal:1","name":"Test Proposal 1","entity_type":"proposal","chain_id":1}
{"index":{"_id":"10"}}
{"id":"dao:test:proposal:2","name":"Test Proposal 2","entity_type":"proposal","chain_id":1}
'

# Refresh the index to make documents searchable
curl -X POST "http://localhost:9200/entities_v1/_refresh"

# Verify documents were added
echo -e "\n=== Document count in entities_v1 ==="
curl -s "http://localhost:9200/entities_v1/_count" | jq

# Search to see the documents
echo -e "\n=== Sample documents ==="
curl -s "http://localhost:9200/entities_v1/_search?size=3&pretty" | jq '.hits.hits[]._source'
```

### Step 6: Update Alias to Point to v1

```bash
# Point the entities alias to v1 (from search-admin directory)
cargo run -- update-alias --version 1
```

### Step 7: List Indices

```bash
# Should show entities_v1 with the alias pointing to it
cargo run -- list-indices
```

### Step 8: Deploy Test Search Indexer

```bash
# Create a test search-indexer deployment
kubectl apply -f - <<EOF
apiVersion: apps/v1
kind: Deployment
metadata:
  name: search-indexer
  namespace: search
spec:
  replicas: 1
  selector:
    matchLabels:
      app: search-indexer
  template:
    metadata:
      labels:
        app: search-indexer
    spec:
      containers:
      - name: search-indexer
        image: busybox:latest
        command: ["sh", "-c", "echo 'Search indexer running with version '$$ENTITIES_INDEX_VERSION && sleep infinity"]
        env:
        - name: ENTITIES_INDEX_VERSION
          value: "1"
EOF

# Wait for it to be ready
# First wait for pod to be created
until kubectl get pod -l app=search-indexer -n search 2>/dev/null | grep -q search-indexer; do
  echo "Waiting for search-indexer pod to be created..."
  sleep 2
done
kubectl wait --for=condition=ready pod -l app=search-indexer -n search --timeout=60s

# Verify deployment exists
kubectl get deployment search-indexer -n search
```

### Step 9: Run Full Migration from v1 to v2

The full-migration command requires Kubernetes API access. Use the test script:

```bash
# Go to test directory
cd ../../search-indexer-deploy/tests

# Run the full migration test
./test-full-migration.sh 1 2

# The migration will:
#   1. Create entities_v2
#   2. Stop search-indexer
#   3. Reindex all documents from v1 to v2
#   4. Update alias to point to v2
#   5. Start search-indexer with version 2
```

### Step 10: Verify Migration

```bash
# Back to search-admin directory for cargo commands
cd ../../search-admin

# List indices (should see both v1 and v2, with alias pointing to v2)
cargo run -- list-indices

# Verify document counts match (port-forward should still be running)
# Check v1 count
echo "=== v1 count ==="
curl -s "http://localhost:9200/entities_v1/_count" | jq

# Check v2 count (should match v1)
echo "=== v2 count ==="
curl -s "http://localhost:9200/entities_v2/_count" | jq

# Verify alias points to v2
echo "=== Aliases ==="
curl -s "http://localhost:9200/_cat/aliases?v"

# Search via alias (should use v2)
echo "=== Search via alias ==="
curl -s "http://localhost:9200/entities/_search?size=2&pretty" | jq '.hits.hits[]._source'
```

### Step 11: Test Delete Old Index

```bash
# Delete the old v1 index
cargo run -- delete-index --version 1 --confirm --yes

# Verify only v2 remains
cargo run -- list-indices
```

### Step 12: Test Migration to v3

```bash
# Run another migration (from tests directory)
cd ../../search-indexer-deploy/tests
./test-full-migration.sh 2 3

# Verify (from search-admin directory)
cd ../../search-admin
cargo run -- list-indices
```

## Testing Individual Commands

You can test individual commands using cargo (ensure OPENSEARCH_URL is set and port-forward is running):

### Test create-index

```bash
cd search-admin
cargo run -- create-index --version 99
cargo run -- list-indices
```

### Test reindex (synchronous)

```bash
# Reindex with wait-for-completion
cargo run -- reindex --source-version 2 --target-version 3 --wait-for-completion
```

### Test update-alias

```bash
cargo run -- update-alias --version 3
cargo run -- list-indices
```

### Test delete-index

```bash
cargo run -- delete-index --version 99 --confirm --yes
cargo run -- list-indices
```

## Cleanup

When you're done testing:

```bash
# Kill port-forward if still running
kill $PF_PID 2>/dev/null || true

# Delete the namespace
kubectl delete namespace search

# Delete kind cluster
kind delete cluster --name search-test
```

## Automated Test Scripts

For automated testing, use the provided scripts:

### Setup Environment

```bash
# From repository root
./search-indexer-deploy/tests/setup-test-environment.sh
```

This creates a complete test environment with OpenSearch, test data, and a mock search-indexer deployment.

### Test Full Migration

```bash
# Test migration from v1 to v2
./search-indexer-deploy/tests/test-full-migration.sh 1 2
```

This runs the full migration workflow using Kubernetes Jobs.

## Troubleshooting

### OpenSearch pod is pending

```bash
# Check resource availability
kubectl describe pod -n search -l app=opensearch

# Check node status
kubectl get nodes
kubectl describe node search-test-control-plane
```

### Image not found

```bash
# Verify the image is in kind
docker exec -it search-test-control-plane crictl images | grep search-admin

# Reload if needed
kind load docker-image search-admin:local --name search-test
```

### Pods can't pull images

```bash
# Verify imagePullPolicy is set to Never
kubectl get pod -n search -l app=opensearch -o yaml | grep -A2 imagePullPolicy
```

### Port-forward fails

```bash
# Check if pod is running
kubectl get pods -n search

# Get pod logs
kubectl logs -n search -l app=opensearch

# Try direct pod port-forward
kubectl port-forward -n search pod/<opensearch-pod-name> 9200:9200
```

### Cluster won't start

```bash
# Delete and recreate
kind delete cluster --name search-test
kind create cluster --name search-test
```

### OpenSearch won't start

```bash
# Check logs
kubectl logs -n search -l app=opensearch

# Common issues:
# - Not enough memory: kind uses Docker resources
# - Port conflicts: Check if another process is using port 9200
```

## Tips for Iterating

When making changes to the search-admin code:

### Fast iteration with cargo (recommended for development)

```bash
# Make changes to the code
# Run immediately without rebuilding Docker
cd search-admin
cargo run -- <your-command>
```

### Testing with Kubernetes Jobs

```bash
# 1. Rebuild the image (from repo root)
docker build -f search-admin/Dockerfile -t search-admin:local .

# 2. Reload into kind
kind load docker-image search-admin:local --name search-test

# 3. Test your changes
./search-indexer-deploy/tests/test-full-migration.sh 1 2
```

## What Gets Tested

This local testing workflow validates:

- ✅ Building the search-admin Docker image (for Kubernetes Job testing)
- ✅ Running commands directly with cargo (for rapid development)
- ✅ Index creation with proper mappings
- ✅ Document insertion and indexing
- ✅ Reindex operation (synchronous)
- ✅ Alias management and updates
- ✅ Search-indexer deployment orchestration (via Kubernetes Jobs)
- ✅ Full migration workflow (via test-full-migration.sh)
- ✅ Index deletion

## Differences from Production

When testing locally, be aware of:

- **Image source**: Using local images instead of registry
- **Resources**: Lower resource limits
- **Security**: Disabled for simplicity (DISABLE_SECURITY_PLUGIN=true)
- **Scale**: Single-node OpenSearch instead of clustered
- **Data**: Test data instead of production data
- **Secrets**: Manually created instead of managed by ops team
- **Network**: Local kind network instead of production network
