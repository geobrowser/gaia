# Search Indexer Deployment

Kubernetes deployment configurations for the search indexer, OpenSearch, and monitoring stack.

## Local Development

Use docker-compose for local development:

```bash
cd search-indexer-deploy
docker-compose up
```

### Optional: Enable Sentry Telemetry

To test Sentry integration locally, create a `.env` file in the `search-indexer-deploy` directory:

```bash
# .env file for docker-compose
SENTRY_DSN=https://...@o0.ingest.sentry.io/...
SENTRY_ENVIRONMENT=local-docker
SENTRY_TRACES_SAMPLE_RATE=1.0
SENTRY_DEBUG=true
```

Then start the services:

```bash
docker-compose up
```

Without a `.env` file or `SENTRY_DSN`, the indexer defaults to Console backend (stdout logging).

Services:

- **OpenSearch REST API**: `http://localhost:9200`
- **Grafana**: `http://localhost:4040` (admin/admin)
- **Prometheus**: `http://localhost:9090`
- **OpenSearch Exporter**: `http://localhost:9114`

Run the search indexer locally:

```bash
OPENSEARCH_URL=http://localhost:9200 cargo run -p search-indexer --features search-indexer-repository/auto_index_creation
```

Check cluster health:

```bash
curl http://localhost:9200/_cluster/health?pretty
```

## Production

Production runs on Kubernetes and is deployed via GitHub Actions.

The Kubernetes manifests are in `k8s/`.

### Prerequisites

Before deploying, create the required secrets in the `search` namespace:

| Secret                   | Keys                                                                   | Description                                                                     |
| ------------------------ | ---------------------------------------------------------------------- | ------------------------------------------------------------------------------- |
| `kafka-credentials`      | `KAFKA_BROKER`, `KAFKA_USERNAME`, `KAFKA_PASSWORD`, `KAFKA_SSL_CA_PEM` | Managed Kafka connection (see [hermes README](../hermes/README.md) for details) |
| `opensearch-credentials` | `OPENSEARCH_URL`                                                       | OpenSearch connection URL                                                       |
| `search-indexer-secrets` | `SENTRY_DSN`                                                           | Optional, for Sentry telemetry and error tracking                               |

Create the Sentry secret:

```bash
kubectl create secret generic search-indexer-secrets \
  --from-literal=SENTRY_DSN='https://...@o0.ingest.sentry.io/...' \
  --namespace=search
```

### Manual Deployment

```bash
# Apply base configuration
kubectl apply -k search-indexer-deploy/k8s/

# Or using kustomize directly
kustomize build search-indexer-deploy/k8s/ | kubectl apply -f -
```

## Directory Structure

```
search-indexer-deploy/
+-- docker-compose.yaml  # Local development
+-- prometheus.yml       # Prometheus config for docker-compose
+-- grafana/             # Grafana provisioning configs
|   +-- datasources.yml
|   +-- dashboard-providers.yml
|   +-- dashboards/     # Dashboard JSON files
+-- k8s/                 # Kubernetes manifests
    +-- jobs/
    |   +-- README.md           # Index migration documentation
    +-- production/
    |   +-- kustomization.yaml
    |   +-- namespace.yaml      # namespace: search
    |   +-- search-indexer.yaml
    |   +-- jobs/               # Production migration jobs (ENVIRONMENT=production)
    +-- staging/
    |   +-- namespace.yaml      # namespace: search-staging
    |   +-- search-indexer.yaml
    |   +-- jobs/               # Staging migration jobs (ENVIRONMENT=staging)
    +-- v2/                     # gaia-v2 / testnet (shared namespace gaia-v2, bootstrapped via deploy-v2)
        +-- search-indexer.yaml
        +-- jobs/               # Testnet migration jobs (ENVIRONMENT=testnet)
```

## Index Migrations

OpenSearch index migrations are managed using the [search-admin](../search-admin/) tool, which runs as Kubernetes Jobs.

**Important:** Jobs are separated by environment:

- **Production:** `k8s/production/jobs/` (namespace: `search`, no index prefix)
- **Staging:** `k8s/staging/jobs/` (namespace: `search-staging`, `staging_` prefix)
- **Testnet (gaia-v2):** `k8s/v2/jobs/` (namespace: `gaia-v2`, `testnet_` prefix)

**For full migration workflows and step-by-step instructions, see:**

- **[k8s/jobs/README.md](k8s/jobs/README.md)** - Complete guide for running index migrations
- **[search-admin/README.md](../search-admin/README.md)** - CLI tool documentation and commands

The jobs automate the entire migration process including creating indices, stopping the indexer, reindexing data, updating aliases, and restarting with the new version.

## Resource Configuration

| Environment | OpenSearch RAM | OpenSearch Heap | Grafana RAM | Prometheus RAM |
| ----------- | -------------- | --------------- | ----------- | -------------- |
| Production  | 6 GB           | 3 GB            | —           | —              |
| Local       | 2 GB           | 1 GB            | 1 GB        | 256 MB         |

## Services

| Service              | Port                   | Description                                    |
| -------------------- | ---------------------- | ---------------------------------------------- |
| OpenSearch REST API  | 9200                   | Search and indexing API                        |
| OpenSearch Transport | 9300                   | Inter-node communication                       |
| Grafana              | 4040                   | Metrics dashboards (local only)                |
| Prometheus           | 9090                   | Metrics collection and querying (local only)   |
| OpenSearch Exporter  | 9114                   | Prometheus metrics exporter                    |

## Monitoring Stack

In production there is no separate search monitoring stack: `opensearch-exporter` runs in the `monitoring` namespace ([`monitoring/k8s/opensearch-exporter.yaml`](../monitoring/k8s/opensearch-exporter.yaml)) and the cluster-wide Prometheus scrapes it.

The local `docker-compose.yaml` stack includes:

- **OpenSearch Exporter**: Exports OpenSearch metrics in Prometheus format using the [prometheus-community/elasticsearch_exporter](https://github.com/prometheus-community/elasticsearch_exporter)
- **Prometheus**: Scrapes metrics from the exporter and stores time-series data
- **Grafana**: Pre-configured with an OpenSearch Overview dashboard showing:
  - Search QPS and latency
  - Document counts and indexing rates
  - Cluster health and shard status
  - JVM heap and GC metrics
  - CPU, memory, and filesystem utilization
  - Thread pool queues and rejections
  - Circuit breaker status

## Environment Variables

The search-indexer binary uses these environment variables:

### Core Configuration

| Variable                 | Description                                                    | Default                 |
| ------------------------ | -------------------------------------------------------------- | ----------------------- |
| `OPENSEARCH_URL`         | OpenSearch REST endpoint                                       | `http://localhost:9200` |
| `INDEX_ALIAS`            | Index alias name                                               | `entities`              |
| `ENTITIES_INDEX_VERSION` | Index version number                                           | `0`                     |
| `KAFKA_BROKER`           | Kafka broker address                                           | `localhost:9092`        |
| `KAFKA_GROUP_ID`         | Consumer group ID                                              | `search-indexer`        |
| `KAFKA_USERNAME`         | Kafka SASL username (required for managed Kafka)               | -                       |
| `KAFKA_PASSWORD`         | Kafka SASL password (required for managed Kafka)               | -                       |
| `KAFKA_SSL_CA_PEM`       | Kafka CA certificate PEM (required for managed Kafka with SSL) | -                       |
| `RUST_LOG`               | Log level                                                      | `search_indexer=info`   |

### Telemetry (Sentry)

| Variable                    | Description                                     | Default |
| --------------------------- | ----------------------------------------------- | ------- |
| `SENTRY_DSN`                | Sentry project DSN (enables Sentry when set)    | -       |
| `SENTRY_ENVIRONMENT`        | Environment tag (e.g., "staging", "production") | -       |
| `SENTRY_RELEASE`            | Release version (e.g., "search-indexer@1.0.0")  | -       |
| `SENTRY_TRACES_SAMPLE_RATE` | Trace sampling rate 0.0-1.0                     | `1.0`   |
| `SENTRY_SEND_DEFAULT_PII`   | Include PII in events                           | `false` |
| `SENTRY_DEBUG`              | Enable debug mode (logs spans to stdout)        | `false` |

See the main [search-indexer README](../search-indexer/README.md) for detailed telemetry documentation.

## Security Notes

⚠️ **The default configuration disables OpenSearch security for development.**

For production, you should:

1. Enable the OpenSearch security plugin
2. Configure TLS certificates
3. Set up authentication
4. Use Kubernetes secrets for credentials
