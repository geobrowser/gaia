# Search Indexer Deployment

Kubernetes deployment configurations for the search index and monitoring stack.

## Local Development

Use docker-compose for local development:

```bash
cd search-indexer-deploy
docker-compose up
```

Services:
- **OpenSearch REST API**: `http://localhost:9200`
- **OpenSearch Dashboards**: `http://localhost:5601`
- **Grafana**: `http://localhost:4040` (admin/admin)
- **Prometheus**: `http://localhost:9090`
- **OpenSearch Exporter**: `http://localhost:9114`

Check cluster health:
```bash
curl http://localhost:9200/_cluster/health?pretty
```

## Production

Production runs on Kubernetes and is deployed via GitHub Actions.

The Kubernetes manifests are in `k8s/`.

### Prerequisites

Before deploying, create the required secrets in the `search` namespace:

| Secret | Keys | Description |
|--------|------|-------------|
| `kafka-credentials` | `KAFKA_BROKER`, `KAFKA_USERNAME`, `KAFKA_PASSWORD`, `KAFKA_SSL_CA_PEM` | Managed Kafka connection (see [hermes README](../hermes/README.md) for details) |
| `grafana-credentials` | `ADMIN_USER`, `ADMIN_PASSWORD` | Grafana admin login |

See [k8s-secrets-isolation.md](../docs/k8s-secrets-isolation.md) for the secrets strategy.

### Manual Deployment

```bash
# Apply base configuration
kubectl apply -k search-indexer-deploy/k8s/

# Or using kustomize directly
kustomize build search-indexer-deploy/k8s/ | kubectl apply -f -
```

### Accessing Grafana

Grafana is exposed publicly via NodePort on port `30440`. Access it at:

```
http://<node-ip>:30440
```

To find your node IP:
```bash
kubectl get nodes -o wide
```

**Note**: Grafana is accessible over HTTP (not HTTPS). For production use, consider adding TLS/HTTPS or restricting access via firewall rules.

The credentials are set via the `grafana-credentials` secret created in the prerequisites step above.

## Directory Structure

```
search-indexer-deploy/
├── docker-compose.yaml  # Local development
├── prometheus.yml       # Prometheus config for docker-compose
├── grafana/             # Grafana provisioning configs
│   ├── datasources.yml
│   ├── dashboard-providers.yml
│   └── dashboards/     # Dashboard JSON files
└── k8s/                 # Kubernetes manifests (production)
    ├── kustomization.yaml
    ├── namespace.yaml
    └── monitoring.yaml      # Prometheus + Grafana + OpenSearch Exporter
```

## Resource Configuration

| Environment | OpenSearch RAM | OpenSearch Heap | Dashboards RAM | Grafana RAM | Prometheus RAM |
|-------------|----------------|-----------------|----------------|-------------|----------------|
| Production  | 6 GB           | 3 GB            | 1 GB           | 2 GB        | 512 MB         |
| Local       | 2 GB           | 1 GB            | 512 MB         | 1 GB        | 256 MB         |

## Services

| Service | Port | Description |
|---------|------|-------------|
| OpenSearch REST API | 9200 | Search and indexing API |
| OpenSearch Transport | 9300 | Inter-node communication |
| OpenSearch Dashboards | 5601 | Web UI for OpenSearch queries |
| Grafana | 4040 (NodePort: 30440) | Metrics dashboards (HTTP, publicly accessible) |
| Prometheus | 9090 | Metrics collection and querying |
| OpenSearch Exporter | 9114 | Prometheus metrics exporter |

## Monitoring Stack

The monitoring stack includes:

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

## Security Notes

⚠️ **The default configuration disables OpenSearch security for development.**

For production, you should:
1. Enable the OpenSearch security plugin
2. Configure TLS certificates
3. Set up authentication
4. Use Kubernetes secrets for credentials (✅ Grafana credentials are already using secrets)

### Grafana Security (Production)

- Grafana is exposed via NodePort on port `30440` over HTTP
- Credentials are stored in Kubernetes secrets (see Prerequisites section)
- For production, consider:
  - Adding TLS/HTTPS (via Ingress + cert-manager or Load Balancer)
  - Restricting access via firewall rules or network policies
  - Using OAuth/SSO authentication
