# Hermes Infrastructure

Kafka infrastructure for the Hermes event stream service.

## Local Development

Use docker-compose for local development:

```bash
docker-compose up
```

Services:
- **Kafka broker**: `localhost:9092`
- **Kafka UI**: http://localhost:8080

Run the producer:
```bash
cd hermes-producer
KAFKA_BROKER=localhost:9092 cargo run
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
kubectl logs -n kafka -l app=hermes-producer --tail=50 -f
```

## Structure

```
hermes/
├── docker-compose.yaml  # Local development
└── k8s/                 # Kubernetes manifests (production)
```
