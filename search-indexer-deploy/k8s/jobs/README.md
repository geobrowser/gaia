# OpenSearch Index Management

Unified CLI tool for managing OpenSearch index migrations. Uses a Rust-based CLI (`search-admin`) built via CI/CD and executed through kubectl.

## Quick Start

```bash
cd search-indexer-deploy/k8s/jobs

# Run full migration (example from v2 to v3)
./search-admin.sh full-migration 2 3
```

This will:
1. Create the new index (entities_v3)
2. Stop the search-indexer (avoids overwriting the new index)
3. Reindex all data (v2 → v3)
4. Start the search-indexer with the new version

Each step requires confirmation with detailed configuration display.

## Configuration Options

### Kubeconfig

If you need to use a custom kubeconfig file:

```bash
# Option 1: Environment variable
export KUBECONFIG=/path/to/kubeconfig.yaml
./search-admin.sh full-migration 2 3

# Option 2: Command-line flag
./search-admin.sh --kubeconfig /path/to/kubeconfig.yaml full-migration 2 3
```

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `KUBECONFIG` | Path to kubeconfig file | Default kubectl config |
| `RUST_LOG` | Log level (debug, info, warn, error) | `info` |

Examples:
```bash
# Enable debug logging
RUST_LOG=debug ./search-admin.sh full-migration 2 3
```

## How It Works

### Docker Image from CI/CD

The `search-admin.sh` script uses a Docker image built and pushed by GitHub Actions:

1. **Code Changes**: Make changes to `search-admin-cli/` or `search-indexer-repository/`
2. **Merge to Main**: Push to main branch triggers `.github/workflows/search-admin-deploy.yml`
3. **CI/CD Build**: GitHub Actions builds the Docker image and pushes to `registry.digitalocean.com/geo/search-admin:latest`
4. **Local Execution**: Run `./search-admin.sh` commands which use `kubectl run` with the CI/CD-built image

**No local Docker required!** The script runs the CLI via kubectl using the image from the registry.

### What's in the Image

The Docker image contains:
- `search-admin` Rust CLI binary
- Index configuration from `search-indexer-repository/src/opensearch/index_config.rs`
- OpenSearch client libraries
- Type-safe index operations

This ensures:
- ✅ Configuration reuse between indexer and admin tool
- ✅ Type safety from Rust
- ✅ Consistent behavior across environments
- ✅ Everyone uses the same CI/CD-built image

## Available Commands

While `full-migration` is the primary workflow, individual commands are available:

```bash
# Check status of deployment and pods
./search-admin.sh status

# List all indices
./search-admin.sh list-indices

# Create a new index
./search-admin.sh create-index --version 3

# Reindex data
./search-admin.sh reindex --source-version 2 --target-version 3

# Stop/start indexer
./search-admin.sh stop-indexer
./search-admin.sh start-indexer 3

# Delete old index (after verification)
./search-admin.sh delete-index --version 2 --confirm
```

## Prerequisites

- kubectl configured with access to the cluster
- Access to the `search` namespace
- Write/delete permissions on Kubernetes resources
- `opensearch-credentials` secret exists in the namespace

## Troubleshooting

### Image Pull Errors

```
Error: Failed to pull image "registry.digitalocean.com/geo/search-admin:latest"
```

**Solution**:
- Verify the GitHub Actions workflow completed successfully
- Check that the Kubernetes cluster has pull access to the registry

### Permission Denied

```bash
# Make script executable
chmod +x search-admin.sh
```

### Secret Not Found

```
Error: secrets "opensearch-credentials" not found
```

**Solution**:
```bash
kubectl create secret generic opensearch-credentials \
  --from-literal=OPENSEARCH_URL=https://your-opensearch-url:9200 \
  -n search
```

### Operation Cancelled

All operations require explicit confirmation. If you see:
```
⚠  Operation cancelled by user
```

You either:
- Typed something other than "yes" at the confirmation prompt
- Pressed Ctrl+C to cancel

This is intentional safety behavior. Review the configuration details shown in the confirmation prompt before proceeding.

## Safety Features

- ✅ **Confirmation Required**: Every destructive operation requires typing "yes" to proceed
- ✅ **Configuration Display**: Shows namespace, kubeconfig, and operation details before execution
- ✅ **Pre-flight Checks**: Validates index existence and deployment status
- ✅ **Detailed Logging**: All operations log progress and results
- ✅ **Error Handling**: Clear error messages with suggested fixes

## Additional Documentation

For more detailed information:
- **CLI Tool**: See `search-admin-cli/README.md` for command reference
- **CI/CD Pipeline**: See `.github/workflows/search-admin-deploy.yml`
- **Index Config**: See `search-indexer-repository/src/opensearch/index_config.rs`
