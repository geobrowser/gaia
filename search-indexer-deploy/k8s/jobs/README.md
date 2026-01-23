# OpenSearch Index Management

Unified CLI tool for managing OpenSearch index migrations. Uses a Rust-based CLI (`search-admin`) built via CI/CD and executed through kubectl.

## Prerequisites

Contact your Kubernetes administrator to obtain search admin credentials with permissions to run migrations in the `search` namespace.

## Quick Start

```bash
cd search-indexer-deploy/k8s/jobs

# Run the full migration (e.g., from v2 to v3)
./run-full-migration.sh 2 3
```

This will:
1. Create the new index (entities_v3)
2. Stop the search-indexer (avoids overwriting the new index)
3. Reindex all data (v2 → v3)
4. Update the alias to point to the new index
5. Start the search-indexer with the new version

All steps are orchestrated automatically by the Rust CLI running in Kubernetes.

## Configuration Options

### Environment Variables

The full-migration job supports several environment variables:

| Variable | Description | Default |
|----------|-------------|---------|
| `OPENSEARCH_URL` | OpenSearch endpoint URL | Read from `search-indexer-secrets` secret |
| `INDEX_ALIAS` | Index alias name | `entities` |
| `SOURCE_VERSION` | Source index version | Required (set in job YAML) |
| `TARGET_VERSION` | Target index version | Required (set in job YAML) |
| `RUST_LOG` | Log level (debug, info, warn, error) | `info` |

Examples:
```bash
# Get OpenSearch URL from the secret
kubectl get secret search-indexer-secrets -n search -o jsonpath='{.data.OPENSEARCH_URL}' | base64 -d

# Enable debug logging by editing the job YAML
# Add to env section:
# - name: RUST_LOG
#   value: "debug"
```

## How It Works

### Docker Image from CI/CD

The migration jobs use a Docker image built and pushed by GitHub Actions:

1. **Code Changes**: Make changes to `search-admin/` or `search-indexer-repository/`
2. **Merge to Main**: Push to main branch triggers `.github/workflows/search-admin-build.yml`
3. **CI/CD Build**: GitHub Actions builds the Docker image and pushes to `registry.digitalocean.com/geo/search-admin:latest`
4. **Kubernetes Jobs**: Apply job YAML files which use the CI/CD-built image

### What's in the Image

The Docker image contains:
- `search-admin` Rust CLI binary with Kubernetes client
- Index configuration from `search-indexer-repository/src/opensearch/index_config.rs`
- OpenSearch client libraries
- Kubernetes client (kube-rs) for deployment management
- Type-safe index operations

This ensures:
- ✅ Configuration reuse between indexer and admin tool
- ✅ Type safety from Rust
- ✅ Consistent behavior across environments
- ✅ Everyone uses the same CI/CD-built image
- ✅ Self-contained migrations (no shell scripts required)

## Available Commands

### Full Migration (Recommended)

Run the complete migration workflow as a Kubernetes Job:

```bash
# Run the migration (e.g., from v2 to v3)
./run-full-migration.sh 2 3
```

The script will:
- Clean up any previous migration job
- Create a new job with the specified versions
- Automatically follow the logs
- Show verification commands when complete

**Alternative: Edit and apply YAML directly**

If you prefer to customize the job configuration:
```bash
# Edit full-migration-job.yaml to set SOURCE_VERSION and TARGET_VERSION
kubectl apply -f full-migration-job.yaml
kubectl logs -n search -f job/opensearch-full-migration
```

### Individual Commands

For debugging or manual operations, individual commands are available as separate jobs:

```bash
# List all indices and aliases
kubectl apply -f list-indices-job.yaml
kubectl logs -n search -f job/opensearch-list-indices

# Create a new index (for testing)
# Edit create-index-job.yaml to set INDEX_VERSION
kubectl apply -f create-index-job.yaml
kubectl logs -n search -f job/opensearch-create-index

# Delete old index (after verification)
# Edit delete-index-job.yaml to set INDEX_VERSION and CONFIRM_DELETE=true
kubectl apply -f delete-index-job.yaml
kubectl logs -n search -f job/opensearch-delete-index
```

## Troubleshooting

### Image Pull Errors

```
Error: Failed to pull image "registry.digitalocean.com/geo/search-admin:latest"
```

**Solution**:
- Verify the GitHub Actions workflow completed successfully
- Check that the Kubernetes cluster has pull access to the registry

### Secret Not Found

```
Error: secrets "search-indexer-secrets" not found
```

**Solution**:
```bash
kubectl create secret generic search-indexer-secrets \
  --from-literal=OPENSEARCH_URL=https://your-opensearch-url:9200 \
  -n search
```

### Job Hangs or Fails

If jobs hang or fail:

**Check OpenSearch connectivity**:
```bash
# Get the OpenSearch URL from the secret
OPENSEARCH_URL=$(kubectl get secret search-indexer-secrets -n search -o jsonpath='{.data.OPENSEARCH_URL}' | base64 -d)
echo $OPENSEARCH_URL

# Test connectivity from within the cluster
kubectl run -it --rm debug --image=curlimages/curl --restart=Never -n search -- curl -v "$OPENSEARCH_URL/_cluster/health"
```

**Check job and pod status**:
```bash
# View jobs in search namespace
kubectl get jobs -n search

# View pods (including completed jobs)
kubectl get pods -n search --show-all

# Check job logs
kubectl logs -n search job/<job-name>

# Check pod events
kubectl describe pod <pod-name> -n search
```

## Safety Features

- ✅ **Pre-flight Checks**: Validates index existence and deployment status before migrations
- ✅ **Detailed Logging**: All operations log progress and results with structured tracing
- ✅ **Error Handling**: Clear error messages with suggested fixes
- ✅ **Type Safety**: Rust CLI ensures type-safe operations and configuration reuse
- ✅ **Kubernetes Jobs**: Declarative job definitions with automatic retry on failure


## Additional Documentation

For more detailed information:
- **CLI Tool**: See `search-admin/README.md` for command reference
- **CI/CD Pipeline**: See `.github/workflows/search-admin-build.yml`
- **Index Config**: See `search-indexer-repository/src/opensearch/index_config.rs`
