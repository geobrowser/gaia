# Search Indexer Deploy Tests

This directory contains test scripts for testing the search-indexer deployment and migration workflows.

## setup-test-environment.sh

Automated end-to-end test script that sets up a complete local test environment for testing index migration workflows.

### What it does

1. Cleans up any existing test environment (deletes old cluster and images)
2. Creates a fresh local kind Kubernetes cluster
3. Deploys OpenSearch in the cluster
4. Creates necessary secrets
5. Builds and loads the search-admin CLI Docker image
6. Creates test indices with sample data (5 test documents)
7. Sets up a test search-indexer deployment
8. Configures the alias to point to the initial index
9. Prepares the environment for testing full migration workflows

**Note:** The script automatically cleans up any previous `search-test` cluster before creating a new one, so you can safely run it multiple times without manual cleanup.

### Usage

From the repository root:

```bash
./search-indexer-deploy/tests/setup-test-environment.sh
```

After the script completes, you can test the full migration workflow:

```bash
# Using the test script (recommended)
./search-indexer-deploy/tests/test-full-migration.sh 1 2

# Or using the kubectl wrapper (interactive)
cd search-admin
./kubectl-search-admin.sh list-indices
./kubectl-search-admin.sh full-migration 1 2
```

### Requirements

- **Docker**: Container runtime
- **kind**: Local Kubernetes clusters using Docker containers
  ```bash
  brew install kind
  ```
- **kubectl**: Kubernetes command-line tool
- **jq** (optional): JSON processor for prettier output
  ```bash
  brew install jq
  ```

### What gets tested

This test validates the complete deployment workflow:

- ✅ Building search-admin Docker image
- ✅ Running search-admin commands via kubectl
- ✅ Index creation with proper mappings
- ✅ Document insertion and indexing
- ✅ Alias management (create, update)
- ✅ Reindex operations
- ✅ Search-indexer deployment orchestration
- ✅ Full migration workflow (create → stop → reindex → update alias → start)
- ✅ Index deletion

### Cleanup

When you're done testing:

```bash
# Delete just the namespace (keeps cluster for reuse)
kubectl delete namespace search

# Or delete the entire cluster
kind delete cluster --name search-test
```

### Running from anywhere

The script uses `REPO_ROOT` to automatically find the repository root, so it works regardless of where you run it from:

```bash
# From repo root
./search-indexer-deploy/tests/setup-test-environment.sh

# From tests directory
cd search-indexer-deploy/tests
./setup-test-environment.sh

# From anywhere else
/path/to/repo/search-indexer-deploy/tests/setup-test-environment.sh
```

### Test environment details

**Cluster:**
- Single-node kind cluster named `search-test`

**OpenSearch:**
- Single-node deployment (development mode)
- Security disabled for simplicity
- 512MB-1GB memory allocation
- Accessible at `http://opensearch.search.svc.cluster.local:9200` within cluster

**Test data:**
- 5 test documents in `entities_v1` index
- Mix of address and proposal entity types
- Alias `entities` pointing to `entities_v1`

**Search indexer:**
- Mock deployment (busybox container)
- Used to test start/stop orchestration
- Configured with `ENTITIES_INDEX_VERSION=1`

### Differences from production

When testing locally, be aware of:

- **Image source**: Using locally-built images instead of registry
- **Resources**: Lower resource limits suitable for local development
- **Security**: Disabled for simplicity (DISABLE_SECURITY_PLUGIN=true)
- **Scale**: Single-node OpenSearch instead of clustered deployment
- **Data**: Test data instead of production data
- **Secrets**: Manually created instead of managed by ops team
- **Network**: Local kind network instead of production network

## Troubleshooting

### OpenSearch pod is pending

```bash
# Check resource availability
kubectl describe pod -n search -l app=opensearch

# Check node status
kubectl get nodes
```

### Image not found

```bash
# Verify the image is in kind
docker exec -it search-test-control-plane crictl images | grep search-admin

# Reload if needed
kind load docker-image search-admin:local --name search-test
```

### Port-forward fails

```bash
# Check if pod is running
kubectl get pods -n search

# Get pod logs
kubectl logs -n search -l app=opensearch

# Try direct pod port-forward
POD_NAME=$(kubectl get pod -n search -l app=opensearch -o jsonpath='{.items[0].metadata.name}')
kubectl port-forward -n search pod/$POD_NAME 9200:9200
```

### Cluster won't start

```bash
# Delete and recreate
kind delete cluster --name search-test
kind create cluster --name search-test
```

### Script keeps cleaning up my cluster

The script automatically deletes any existing `search-test` cluster at the start. If you want to preserve an existing cluster:

1. Rename your existing cluster before running the script:
   ```bash
   # This won't work - kind doesn't support renaming
   # Instead, use a different cluster name or skip the automated script
   ```

2. Or run the setup steps manually (see [LOCAL_TESTING.md](../../search-admin/LOCAL_TESTING.md))

3. Or comment out the cleanup section in the script (lines 7-20)

## See Also

- [../k8s/jobs/README.md](../k8s/jobs/README.md) - Production deployment guide
- [../../search-admin/LOCAL_TESTING.md](../../search-admin/LOCAL_TESTING.md) - Detailed local testing guide with manual steps
- [../../search-admin/README.md](../../search-admin/README.md) - Search-admin CLI documentation
