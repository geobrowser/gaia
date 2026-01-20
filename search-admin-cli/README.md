# search-admin-cli

A Rust CLI tool for managing OpenSearch indices. This tool provides commands for creating, reindexing, deleting, and monitoring OpenSearch indices used by the search-indexer.

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

**GitHub Actions:** `.github/workflows/search-admin-deploy.yml`
- Triggers on push to `main` when search-admin-cli files change
- Builds and pushes: `registry.digitalocean.com/geo/search-admin:latest` and `:<git-sha>`
- Build time: ~5-10 minutes

### Quick Start

```bash
# Make changes, commit to main (via PR or direct push)
git add search-admin-cli/
git commit -m "feat: improve command"
git push origin main

# Wait for CI/CD to build (~5-10 min)

# Run commands using the CI/CD-built image (no Docker needed!)
cd search-indexer-deploy/k8s/jobs
./search-admin.sh list-indices
./search-admin.sh create-index --version 3
```

### Local Development

```bash
# Build and test locally (optional)
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

# Reindex from v2 to v3 (async)
search-admin reindex --source-version 2 --target-version 3

# Reindex synchronously (wait for completion)
search-admin reindex --source-version 2 --target-version 3 --wait-for-completion

# Monitor a reindex task
search-admin monitor-reindex --task-id "oTUltX64Vrzdnr4Z0-jU2w:123"

# List all indices
search-admin list-indices

# List with detailed information
search-admin list-indices --detailed

# Delete an old index (requires --confirm and interactive confirmation)
search-admin delete-index --version 2 --confirm

# Delete non-interactively (use with caution!)
search-admin delete-index --version 2 --confirm --yes
```

### Via kubectl wrapper (Recommended)

The easiest way to run commands in production:

```bash
cd search-indexer-deploy/k8s/jobs

# All commands use CI/CD-built image automatically
./search-admin.sh list-indices
./search-admin.sh create-index --version 3
./search-admin.sh reindex --source-version 2 --target-version 3
./search-admin.sh delete-index --version 2 --confirm
./search-admin.sh status  # Show deployment and indices
```

The script:
- Retrieves OpenSearch URL from Kubernetes secrets
- Creates temporary pod with the latest image
- Streams output to your terminal
- Auto-cleans up when done

### Via Kubernetes Jobs

For scheduled or long-running operations:

```bash
# Apply job YAML files from search-indexer-deploy/k8s/jobs/
kubectl apply -f create-index-job.yaml
kubectl apply -f reindex-job.yaml
kubectl logs -n search job/opensearch-reindex -f
```

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `OPENSEARCH_URL` | OpenSearch connection URL | `http://localhost:9200` |
| `INDEX_ALIAS` | Index alias name | `entities` |
| `RUST_LOG` | Log level (trace, debug, info, warn, error) | `info` |

## Commands

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
# Async reindex (recommended for large indices)
search-admin reindex --source-version 2 --target-version 3

# Sync reindex (recommended for small indices)
search-admin reindex --source-version 2 --target-version 3 --wait-for-completion
```

**What it does:**
1. Verifies both source and target indices exist
2. Gets document count from source index
3. Starts reindex operation
4. Returns task ID (async) or waits for completion (sync)
5. Prints next steps

### monitor-reindex

Monitor an asynchronous reindex task.

```bash
search-admin monitor-reindex \
  --task-id <TASK_ID> \
  [--poll-interval <SECONDS>] \
  [--max-wait <SECONDS>] \
  [--wait]
```

**Options:**
- `--task-id, -t <TASK_ID>`: Task ID to monitor (required)
- `--poll-interval <SECONDS>`: Poll interval in seconds (default: 10)
- `--max-wait <SECONDS>`: Maximum wait time in seconds, 0 for unlimited (default: 3600)
- `--wait`: Block until task completes using OpenSearch wait API

**Example:**
```bash
search-admin monitor-reindex --task-id "oTUltX64Vrzdnr4Z0-jU2w:123" --wait
```

**What it does:**
1. Polls the task status at the specified interval
2. Shows progress (documents created, updated, etc.)
3. Prints final statistics when complete
4. Checks completed tasks if task ID not found in active tasks

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

### Running tests

```bash
cargo test -p search-admin-cli
```

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

## Architecture

The CLI is built on top of the existing search-indexer infrastructure:

```
search-admin-cli
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
- Check GitHub Actions: Go to Actions tab, verify "Deploy Search Admin CLI" workflow succeeded
- Verify build triggered: Changes to `search-admin-cli/**` trigger the workflow
- Wait for build: CI/CD takes ~5-10 minutes
- Check logs: Click on the workflow run to see build logs

### Testing with debug logging

```bash
# For kubectl wrapper
RUST_LOG=debug ./search-admin.sh list-indices

# For local binary
RUST_LOG=debug ./target/release/search-admin list-indices
```

## See Also

- [Index Management Guide](../search-indexer-deploy/k8s/jobs/README.md) - Full migration workflow and deployment guide
- [OpenSearch Reindex API](https://opensearch.org/docs/latest/api-reference/document-apis/reindex/)
