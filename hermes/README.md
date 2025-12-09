# Hermes Infrastructure

Kafka infrastructure for the Hermes event stream service.

## Local Development

Use docker-compose for local development:

```bash
cd hermes
docker-compose up
```

This starts all services:
- **Kafka broker**: `localhost:9092`
- **Kafka UI**: http://localhost:8080
- **hermes-processor**: Processes mock-substream events and publishes to Kafka
- **atlas**: Builds canonical graph from topology events and publishes to Kafka

### Running Services Individually

If you prefer to run the Rust services outside Docker (for faster iteration):

```bash
# Start just Kafka and UI
docker-compose up kafka kafka-ui

# In another terminal, run hermes-processor
KAFKA_BROKER=localhost:9092 cargo run -p hermes-processor

# In another terminal, run atlas
KAFKA_BROKER=localhost:9092 KAFKA_TOPIC=topology.canonical cargo run -p atlas
```

### Rebuilding Images

After code changes, rebuild the Docker images:

```bash
docker-compose build hermes-processor atlas
docker-compose up
```

## Production

Production runs on DigitalOcean Managed Kafka and is deployed to DigitalOcean Kubernetes via GitHub Actions.

The Kubernetes manifests are in `k8s/`.

### Environment Variables

Both `hermes-processor` and `atlas` support the following environment variables:

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `KAFKA_BROKER` | No | `localhost:9092` | Kafka bootstrap server address |
| `KAFKA_USERNAME` | No | - | SASL username for managed Kafka authentication |
| `KAFKA_PASSWORD` | No | - | SASL password for managed Kafka authentication |
| `KAFKA_SSL_CA_PEM` | No | - | CA certificate (PEM format) for SSL verification |
| `KAFKA_TOPIC` | No | `topology.canonical` | Output topic (atlas only) |

When `KAFKA_USERNAME` and `KAFKA_PASSWORD` are both set, the producers automatically enable SASL/SSL authentication (required for DigitalOcean Managed Kafka). When unset, plaintext connections are used (for local development).

For managed Kafka, you also need to provide the CA certificate via `KAFKA_SSL_CA_PEM`.

### Manual Access

```bash
# Connect to cluster
doctl kubernetes cluster kubeconfig save <cluster-name>

# View logs
kubectl logs -n kafka -l app=hermes-processor --tail=50 -f
kubectl logs -n kafka -l app=atlas --tail=50 -f
```

## Structure

```
hermes/
├── docker-compose.yaml  # Local development
└── k8s/                 # Kubernetes manifests (production)
```
