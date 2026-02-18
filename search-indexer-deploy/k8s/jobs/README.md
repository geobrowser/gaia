# OpenSearch Index Management

Kubernetes Jobs for managing OpenSearch index migrations. Uses a Rust-based tool (`search-admin`) built via CI/CD.

## Directory Structure

Jobs are separated by environment to ensure proper index prefixing:

```
k8s/
├── production/
│   └── jobs/           # Production jobs (namespace: search, no index prefix)
├── staging/
│   └── jobs/           # Staging jobs (namespace: search-staging, staging_ prefix)
└── jobs/
    └── README.md       # This documentation
```

**Important**: The `ENVIRONMENT` variable controls the index naming:
- `ENVIRONMENT=production` → indices use base names (e.g., `entities_v3`)
- `ENVIRONMENT=staging` → indices get `staging_` prefix (e.g., `staging_entities_v3`)

## Prerequisites

Contact your Kubernetes administrator to obtain search admin credentials with permissions to run migrations in the appropriate namespace (`search` for production, `search-staging` for staging).

## Kubeconfig Setup

If using a custom kubeconfig file, either:

**Option 1: Export it** (recommended - cleaner commands below)
```bash
export KUBECONFIG=/path/to/your/kubeconfig.yaml
```

**Option 2: Use --kubeconfig flag** with each kubectl command
```bash
kubectl ... --kubeconfig=/path/to/your/kubeconfig.yaml
```

All examples below assume you've exported `KUBECONFIG`.

## Full Migration (Recommended)

Run a complete migration workflow that orchestrates all steps automatically.

### For Production

```bash
cd k8s/production/jobs
```

### For Staging

```bash
cd k8s/staging/jobs
```

### Step 1: Edit the job YAML to set versions

Edit `full-migration-job.yaml` and update these environment variables:
```yaml
- name: SOURCE_VERSION
  value: "2"  # Change to your source version
- name: TARGET_VERSION
  value: "3"  # Change to your target version
```

### Step 2: Run the migration

**Production:**
```bash
# Clean up any previous migration job
kubectl delete job opensearch-full-migration -n search 2>/dev/null || true

# Apply the job
kubectl apply -f production/jobs/full-migration-job.yaml

# Wait for the pod to be ready
kubectl wait --for=condition=ready pod -l job-name=opensearch-full-migration -n search --timeout=300s

# Follow the logs
kubectl logs -n search -f job/opensearch-full-migration
```

**Staging:**
```bash
# Clean up any previous migration job
kubectl delete job opensearch-full-migration -n search-staging 2>/dev/null || true

# Apply the job
kubectl apply -f staging/jobs/full-migration-job.yaml

# Wait for the pod to be ready
kubectl wait --for=condition=ready pod -l job-name=opensearch-full-migration -n search-staging --timeout=300s

# Follow the logs
kubectl logs -n search-staging -f job/opensearch-full-migration
```

### Step 3: Delete the old index (after verification)

After 3-7 days of stable operation, or when you are confident that the old index is not needed, delete it to free up storage space.

> **WARNING**
> Be very careful here! Make sure you are deleting the **OLD** index version, not the current one.
> Double-check that `INDEX_VERSION` matches your **source** version from Step 1.

Edit `delete-index-job.yaml` and set:
```yaml
- name: INDEX_VERSION
  value: "2"  # IMPORTANT: This should be your OLD version (SOURCE_VERSION from Step 1)
- name: CONFIRM_DELETE
  value: "true"  # Required safety flag
```

Then run:

**Production:**
```bash
kubectl delete job opensearch-delete-index -n search 2>/dev/null || true
kubectl apply -f production/jobs/delete-index-job.yaml
kubectl logs -n search -f job/opensearch-delete-index
```

**Staging:**
```bash
kubectl delete job opensearch-delete-index -n search-staging 2>/dev/null || true
kubectl apply -f staging/jobs/delete-index-job.yaml
kubectl logs -n search-staging -f job/opensearch-delete-index
```

### What it does

The full-migration job will:
1. Create the new index (e.g., `entities_v3` or `staging_entities_v3`)
2. Scale down the search-indexer to 0 replicas (stops indexing)
3. Reindex all data from source → target version
4. Update the alias to point to the new index
5. Scale up the search-indexer with the new `ENTITIES_INDEX_VERSION`

### Verify the migration

**Production:**
```bash
# Check the deployment version
kubectl get deployment search-indexer -n search -o jsonpath='{.spec.template.spec.containers[0].env[?(@.name=="ENTITIES_INDEX_VERSION")].value}'

# List indices and aliases
kubectl delete job opensearch-list-indices -n search 2>/dev/null || true
kubectl apply -f production/jobs/list-indices-job.yaml
kubectl logs -n search -f job/opensearch-list-indices
```

**Staging:**
```bash
# Check the deployment version
kubectl get deployment search-indexer -n search-staging -o jsonpath='{.spec.template.spec.containers[0].env[?(@.name=="ENTITIES_INDEX_VERSION")].value}'

# List indices and aliases
kubectl delete job opensearch-list-indices -n search-staging 2>/dev/null || true
kubectl apply -f staging/jobs/list-indices-job.yaml
kubectl logs -n search-staging -f job/opensearch-list-indices
```

## Individual Jobs

For debugging or manual operations. Replace `<env>` with `production` or `staging`, and `<namespace>` with `search` or `search-staging`.

### List Indices and Aliases

```bash
kubectl delete job opensearch-list-indices -n <namespace> 2>/dev/null || true
kubectl apply -f <env>/jobs/list-indices-job.yaml
kubectl logs -n <namespace> -f job/opensearch-list-indices
```

### Create an Index

Edit `<env>/jobs/create-index-job.yaml` to set `INDEX_VERSION`, then:

```bash
kubectl delete job opensearch-create-index -n <namespace> 2>/dev/null || true
kubectl apply -f <env>/jobs/create-index-job.yaml
kubectl logs -n <namespace> -f job/opensearch-create-index
```

### Reindex Data

Copy all documents from one index version to another.

Edit `<env>/jobs/reindex-job.yaml` to set:
- `SOURCE_VERSION` - version to copy from
- `TARGET_VERSION` - version to copy to

Then:

```bash
kubectl delete job opensearch-reindex -n <namespace> 2>/dev/null || true
kubectl apply -f <env>/jobs/reindex-job.yaml
kubectl logs -n <namespace> -f job/opensearch-reindex
```

**Note**: For large indices (millions of documents), this can take significant time.

### Update Alias

Update the alias to point to a different index version.

Edit `<env>/jobs/update-alias-job.yaml` to set `TARGET_VERSION`:

```bash
kubectl delete job opensearch-update-alias -n <namespace> 2>/dev/null || true
kubectl apply -f <env>/jobs/update-alias-job.yaml
kubectl logs -n <namespace> -f job/opensearch-update-alias
```

**Warning**: This switches which index is actively used by the search API. Ensure the target index exists and is properly populated before running this.

### Delete an Index

Edit `<env>/jobs/delete-index-job.yaml` to set:
- `INDEX_VERSION` - version to delete
- `CONFIRM_DELETE=true` - required safety flag

Then:

```bash
kubectl delete job opensearch-delete-index -n <namespace> 2>/dev/null || true
kubectl apply -f <env>/jobs/delete-index-job.yaml
kubectl logs -n <namespace> -f job/opensearch-delete-index
```

## Configuration

### Environment Variables

The jobs support these environment variables:

| Variable | Description | Default |
|----------|-------------|---------|
| `OPENSEARCH_URL` | OpenSearch endpoint URL | Read from `opensearch-credentials` secret |
| `ENVIRONMENT` | Environment name (`staging` or `production`) | Set in job YAML |
| `INDEX_ALIAS` | Base index alias name | `entities` |
| `SOURCE_VERSION` | Source index version (full-migration only) | Required (set in job YAML) |
| `TARGET_VERSION` | Target index version (full-migration only) | Required (set in job YAML) |
| `INDEX_VERSION` | Index version (individual jobs) | Required (set in job YAML) |
| `NAMESPACE` | Kubernetes namespace (full-migration only) | Set in job YAML |
| `RUST_LOG` | Log level (debug, info, warn, error) | `info` |

### Index Naming

The `ENVIRONMENT` variable controls how indices are named:

| Environment | Base Alias | Versioned Index | Alias |
|-------------|------------|-----------------|-------|
| `production` | `entities` | `entities_v3` | `entities` |
| `staging` | `entities` | `staging_entities_v3` | `staging_entities` |

### Getting the OpenSearch URL

**Production:**
```bash
kubectl get secret opensearch-credentials -n search -o jsonpath='{.data.OPENSEARCH_URL}' | base64 -d
```

**Staging:**
```bash
kubectl get secret opensearch-credentials -n search-staging -o jsonpath='{.data.OPENSEARCH_URL}' | base64 -d
```

### Enabling Debug Logging

Edit the job YAML and add to the env section:
```yaml
- name: RUST_LOG
  value: "debug"
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
- `search-admin` Rust binary with Kubernetes client
- Index configuration from `search-indexer-repository/src/opensearch/index_config.rs`
- OpenSearch client libraries
- Kubernetes client (kube-rs) for deployment management
- Type-safe index operations

This ensures:
- Configuration reuse between indexer and admin tool
- Type safety from Rust
- Consistent behavior across environments
- Everyone uses the same CI/CD-built image
- Self-contained migrations (no shell scripts required)

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
Error: secrets "opensearch-credentials" not found
```

**Solution** (use appropriate namespace):
```bash
kubectl create secret generic opensearch-credentials \
  --from-literal=OPENSEARCH_URL=https://your-opensearch-url:9200 \
  -n <namespace>
```

### Job Hangs or Fails

If jobs hang or fail:

**Check OpenSearch connectivity** (production example):
```bash
# Get the OpenSearch URL from the secret
OPENSEARCH_URL=$(kubectl get secret opensearch-credentials -n search -o jsonpath='{.data.OPENSEARCH_URL}' | base64 -d)
echo $OPENSEARCH_URL

# Test connectivity from within the cluster
kubectl run -it --rm debug --image=curlimages/curl --restart=Never -n search -- curl -v "$OPENSEARCH_URL/_cluster/health"
```

**Check job and pod status**:
```bash
# View jobs in namespace
kubectl get jobs -n <namespace>

# View pods (including completed jobs)
kubectl get pods -n <namespace>

# Check job logs
kubectl logs -n <namespace> job/<job-name>

# Check pod events
kubectl describe pod <pod-name> -n <namespace>
```

### Permission Errors

If you get permission errors, verify with your Kubernetes administrator that:
1. Your ServiceAccount has permissions to create/manage jobs
2. The `search-admin` ServiceAccount exists with permissions to manage deployments

## Safety Features

- **Pre-flight Checks**: Validates index existence and deployment status before migrations
- **Detailed Logging**: All operations log progress and results with structured tracing
- **Error Handling**: Clear error messages with suggested fixes
- **Type Safety**: Rust tool ensures type-safe operations and configuration reuse
- **Kubernetes Jobs**: Declarative job definitions with automatic retry on failure
- **Environment Isolation**: Staging and production use separate namespaces and index prefixes

## Additional Documentation

For more detailed information:
- **Search Admin Tool**: See `search-admin/README.md` for command reference
- **CI/CD Pipeline**: See `.github/workflows/search-admin-build.yml`
- **Index Config**: See `search-indexer-repository/src/opensearch/index_config.rs`
