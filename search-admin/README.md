# search-admin

A Rust CLI tool for managing OpenSearch indices. This tool provides commands for creating, reindexing, deleting, and monitoring OpenSearch indices used by the search-indexer. In production, this tool should be executed as Kubernetes Jobs by administrators (see search-indexer-deploy/k8s/jobs/).

## Features

- **Type-safe**: Uses Rust's type system and the existing index configuration from `search-indexer-repository`
- **Reusable**: Leverages the opensearch crate already used by the search-indexer
- **Production-ready**: Comprehensive error handling and logging
- **Kubernetes-native**: Designed to run as Kubernetes Jobs via CI/CD-built images
- **DRY**: Reuses index configuration defined in `search-indexer-repository/src/opensearch/index_config.rs`

## CI/CD Workflow

The CLI is automatically built and deployed via GitHub Actions. No local Docker required!

```
┌─────────────────────┐
│ Developer           │
│ - Edit code         │
│ - Commit & push     │
└──────────┬──────────┘
           │
           ▼
┌─────────────────────┐
│ GitHub (main)       │
│ - Merge PR          │
└──────────┬──────────┘
           │
           ▼
┌─────────────────────┐
│ GitHub Actions      │
│ - Build Docker      │
│ - Push to registry  │
└──────────┬──────────┘
           │
           ▼
┌─────────────────────┐
│ Container Registry  │
│ search-admin:latest │
│ search-admin:<sha>  │
└──────────┬──────────┘
           │
           ▼
┌─────────────────────┐
│ kubectl (local)     │
│ - Run commands      │
│ - Uses :latest      │
└─────────────────────┘
```

**GitHub Actions:** `.github/workflows/search-admin-build.yml`
- Triggers on push to `main` when search-admin files change
- Builds and pushes: `registry.digitalocean.com/geo/search-admin:latest` and `:<git-sha>`
- Build time: ~5-10 minutes

### Quick Start

```bash
# Make changes, commit to main (via PR or direct push)
git add search-admin/
git commit -m "feat: improve command"
git push origin main

# Wait for CI/CD to build (~5-10 min)

# Run migrations using the CI/CD-built image as Kubernetes Jobs
cd search-indexer-deploy/k8s/jobs
./run-full-migration.sh 2 3  # Migrate from v2 to v3
```

### Local Development

```bash
# Build and test locally
cargo build --release --bin search-admin
export OPENSEARCH_URL="http://localhost:9200"
./target/release/search-admin list-indices
```

## Usage

### Command-line

```bash
# Set OpenSearch URL (or use --opensearch-url flag)
export OPENSEARCH_URL="http://localhost:9200"

# Create a new index version
search-admin create-index --version 3

# Reindex from v2 to v3 (wait for completion)
search-admin reindex --source-version 2 --target-version 3 --wait-for-completion

# List all indices
search-admin list-indices

# List with detailed information
search-admin list-indices --detailed

# Delete an old index (requires --confirm and interactive confirmation)
search-admin delete-index --version 2 --confirm

# Delete non-interactively (use with caution!)
search-admin delete-index --version 2 --confirm --yes
```

### Via Kubernetes Jobs (Recommended)

For production migrations, use the full-migration job:

```bash
cd search-indexer-deploy/k8s/jobs

# Run full migration (e.g., from v2 to v3)
./run-full-migration.sh 2 3
```

**Prerequisites:** Contact your Kubernetes administrator for search admin credentials.

For debugging or individual operations:

```bash
# List indices
kubectl apply -f list-indices-job.yaml
kubectl logs -n search -f job/opensearch-list-indices

# Create index (for testing)
kubectl apply -f create-index-job.yaml
kubectl logs -n search -f job/opensearch-create-index

# Delete old index (after verification)
kubectl apply -f delete-index-job.yaml
kubectl logs -n search -f job/opensearch-delete-index
```

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `OPENSEARCH_URL` | OpenSearch connection URL | `http://localhost:9200` |
| `INDEX_ALIAS` | Index alias name | `entities` |
| `RUST_LOG` | Log level (trace, debug, info, warn, error) | `info` |

## Commands

### full-migration

Run the complete migration workflow, orchestrating all steps including Kubernetes deployment management.

```bash
search-admin full-migration \
  --source-version <SOURCE> \
  --target-version <TARGET> \
  [--namespace <NAMESPACE>] \
  [--deployment-name <NAME>]
```

**Options:**
- `--source-version, -s <VERSION>`: Source index version (required)
- `--target-version, -t <VERSION>`: Target index version (required)
- `--namespace <NAMESPACE>`: Kubernetes namespace (default: `search`)
- `--deployment-name <NAME>`: Deployment name to manage (default: `search-indexer`)

**Example:**
```bash
search-admin full-migration --source-version 2 --target-version 3
```

**What it does:**
1. Creates the target index with proper mappings
2. Scales down the search-indexer deployment to 0 replicas
3. Reindexes all data from source to target (waits for completion)
4. Updates the alias to point to the new index
5. Scales up the search-indexer deployment with new `ENTITIES_INDEX_VERSION`

**Requirements:**
- Requires search admin credentials from your Kubernetes administrator

**Note:** This command is designed to run as a Kubernetes Job. For manual migrations, use the individual commands or the shell script wrapper.

### create-index

Create a new versioned index with proper mappings and settings.

```bash
search-admin create-index --version <VERSION> [--skip-if-exists]
```

**Options:**
- `--version, -v <VERSION>`: Index version to create (required)
- `--skip-if-exists`: Skip creation if index already exists (default: fail if exists)

**Example:**
```bash
search-admin create-index --version 3
```

**What it does:**
1. Checks if the versioned index (e.g., `entities_v3`) already exists
2. Creates the index with mappings from `search-indexer-repository/src/opensearch/index_config.rs`
3. Verifies the index was created successfully
4. Prints next steps for the migration process

### reindex

Reindex data from one version to another.

```bash
search-admin reindex \
  --source-version <SOURCE> \
  --target-version <TARGET> \
  [--wait-for-completion] \
  [--batch-size <SIZE>] \
  [--max-docs <COUNT>]
```

**Options:**
- `--source-version, -s <VERSION>`: Source index version (required)
- `--target-version, -t <VERSION>`: Target index version (required)
- `--wait-for-completion`: Wait for reindex to complete (synchronous mode)
- `--batch-size <SIZE>`: Documents per batch (optional)
- `--max-docs <COUNT>`: Maximum documents to reindex (optional, for testing)

**Example:**
```bash
# Synchronous reindex (waits for completion)
search-admin reindex --source-version 2 --target-version 3 --wait-for-completion
```

**Note:** The `full-migration` command uses synchronous reindexing to ensure all steps complete successfully.

**What it does:**
1. Verifies both source and target indices exist
2. Gets document count from source index
3. Starts reindex operation
4. Waits for completion (when --wait-for-completion is used)
5. Prints next steps

### list-indices

List all indices and aliases matching the pattern.

```bash
search-admin list-indices [--detailed] [--pattern <PATTERN>]
```

**Options:**
- `--detailed`: Show detailed information (settings, stats)
- `--pattern <PATTERN>`: Filter by index pattern (default: `entities*`)

**Example:**
```bash
# Simple list
search-admin list-indices

# Detailed view
search-admin list-indices --detailed
```

**What it does:**
1. Lists all indices matching the pattern
2. Shows health, status, document count, and size
3. Shows alias mappings
4. (With --detailed) Shows settings, creation date, and detailed statistics

### delete-index

Delete an old index version.

```bash
search-admin delete-index \
  --version <VERSION> \
  --confirm \
  [--yes]
```

**Options:**
- `--version, -v <VERSION>`: Index version to delete (required)
- `--confirm`: Confirm deletion (required safety flag)
- `--yes`: Skip interactive confirmation prompt

**Example:**
```bash
# Interactive deletion (prompts for confirmation)
search-admin delete-index --version 2 --confirm

# Non-interactive deletion (use with caution!)
search-admin delete-index --version 2 --confirm --yes
```

**Safety features:**
1. Requires `--confirm` flag
2. Verifies index exists
3. Checks if index is currently active (pointed to by alias)
4. Shows index statistics before deletion
5. Requires typing "DELETE" to confirm (unless `--yes` is used)
6. Verifies deletion was successful

## Development

### Running unit tests

```bash
cargo test -p search-admin
```

### Running integration tests

See [search-indexer-deploy/tests/README.md](../search-indexer-deploy/tests/README.md) for end-to-end testing with kind.

### Running locally

```bash
# Port forward to OpenSearch (if running in Kubernetes)
kubectl port-forward -n search svc/opensearch 9200:9200 &

# Run the CLI
export OPENSEARCH_URL="http://localhost:9200"
cargo run --bin search-admin -- list-indices
cargo run --bin search-admin -- create-index --version 3
```

### Building release binary

```bash
cargo build --release --bin search-admin
strip target/release/search-admin  # Reduce binary size
```

### Building Docker image

```bash
# From repository root
docker build -f search-admin/Dockerfile -t search-admin:local .
```

## Architecture

The CLI is built on top of the existing search-indexer infrastructure:

```
search-admin
    ├── Uses: search-indexer-repository
    │   ├── opensearch::index_config (index settings)
    │   ├── opensearch::index_management (index operations)
    │   └── opensearch crate v2.3.0
    └── Uses: search-indexer-shared
        └── Entity types and shared utilities
```

**Benefits:**
- **Single source of truth**: Index configuration is defined once in `search-indexer-repository`
- **Type safety**: Rust's type system ensures correctness
- **Consistency**: CLI uses the same OpenSearch client as the indexer
- **Maintainability**: Changes to index config automatically apply to CLI

## Local Kubernetes Testing

You can test the search-admin CLI and kubernetes workflows locally using kind.

**See [LOCAL_TESTING.md](./LOCAL_TESTING.md) for the complete guide.**

Quick start:

```bash
# Run the automated setup script
./search-indexer-deploy/tests/setup-test-environment.sh

# Test the full migration
./search-indexer-deploy/tests/test-full-migration.sh 1 2
```

The local testing guide includes:
- Setting up a local kind cluster
- Deploying OpenSearch
- Building and loading the search-admin image
- Adding test data
- Running the full migration workflow
- Troubleshooting common issues

## Troubleshooting

### Connection refused

```
Error: Failed to connect to OpenSearch: Connection refused
```

**Solution:**
- Ensure OpenSearch is running
- Check the `OPENSEARCH_URL` environment variable
- Verify network connectivity (use `kubectl port-forward` if needed)

### Index already exists

```
Error: Index entities_v3 already exists
```

**Solution:**
- Use `--skip-if-exists` flag to skip creation
- Or delete the existing index first
- Or use a different version number

### Task not found

```
Error: Task not found in active or completed tasks
```

**Solution:**
- Task may have completed very quickly
- Check if reindex completed successfully by comparing document counts
- Task ID may be incorrect

### Permission denied

```
Error: Failed to create index: 403 Forbidden
```

**Solution:**
- Check OpenSearch authentication/authorization
- Ensure the OpenSearch user has appropriate permissions

### CI/CD image not available

```
Error: Failed to pull image "registry.digitalocean.com/geo/search-admin:latest"
```

**Solution:**
- Check GitHub Actions: Go to Actions tab, verify "Build Search Admin CLI" workflow succeeded
- Verify build triggered: Changes to `search-admin/**` trigger the workflow
- Wait for build: CI/CD takes ~5-10 minutes
- Check logs: Click on the workflow run to see build logs

### Testing with debug logging

```bash
# For Kubernetes Jobs, edit the job YAML to add:
# env:
#   - name: RUST_LOG
#     value: "debug"

# For local binary
RUST_LOG=debug ./target/release/search-admin list-indices
```

## See Also

- [Index Management Guide](../search-indexer-deploy/k8s/jobs/README.md) - Full migration workflow and deployment guide
- [OpenSearch Reindex API](https://opensearch.org/docs/latest/api-reference/document-apis/reindex/)
