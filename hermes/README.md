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

Production runs on DigitalOcean Kubernetes and is deployed via GitHub Actions.

The Kubernetes manifests are in `k8s/`.

### Manual Access

```bash
# Connect to cluster
doctl kubernetes cluster kubeconfig save <cluster-name>

# Kafka UI
kubectl port-forward -n kafka svc/kafka-ui 8080:8080

# View logs
kubectl logs -n kafka -l app=kafka-broker --tail=50 -f
kubectl logs -n kafka -l app=hermes-processor --tail=50 -f
kubectl logs -n kafka -l app=atlas --tail=50 -f
```

## Structure

```
hermes/
├── docker-compose.yaml  # Local development
└── k8s/                 # Kubernetes manifests (production)
```
