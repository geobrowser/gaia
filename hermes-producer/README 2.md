# Hermes - Kafka Event Streaming Service

A Kafka-based event streaming service with protobuf serialization and ZSTD compression.

## Architecture

- **Kafka Broker**: Event streaming platform
- **kafka-ui**: Web interface for monitoring and browsing messages
- **Producer**: Rust-based producer with protobuf + ZSTD compression

## Quick Start

### Local Development (Docker Compose)

**Start Services:**
```bash
docker-compose up -d
```

**Access Services:**
- kafka-ui: http://localhost:8080
- Kafka broker: `localhost:9092`

**Run Producer:**
```bash
cd producer
KAFKA_BROKER=localhost:9092 cargo run
```

**Stop Services:**
```bash
docker-compose down
```

### Local Development (Minikube)

**Start Minikube:**
```bash
minikube start
kubectl config use-context minikube
```

**Deploy:**
```bash
cd k8s
./deploy.sh
```

**Access Services:**
```bash
# In one terminal - port-forward kafka-ui
kubectl port-forward -n kafka svc/kafka-ui 8080:8080

# In another terminal - port-forward broker
kubectl port-forward -n kafka svc/broker 9092:9092

# In third terminal - run producer
cd producer
KAFKA_BROKER=localhost:9092 cargo run
```

**View Logs:**
```bash
kubectl logs -n kafka -l app=kafka-broker --tail=50 -f
kubectl logs -n kafka -l app=kafka-ui --tail=50 -f
```

**Cleanup:**
```bash
cd k8s
./cleanup.sh
```

## Production Deployment (DigitalOcean)

### Prerequisites

1. **DigitalOcean Account** with a Kubernetes cluster
2. **doctl CLI** installed and authenticated
3. **kubectl** configured

### Setup

**Authenticate with DigitalOcean:**
```bash
brew install doctl
doctl auth init
```

**Connect to your cluster:**
```bash
doctl kubernetes cluster kubeconfig save <cluster-name>
kubectl config use-context do-<region>-<cluster-name>
```

### Deploy to Production

**Deploy all services:**
```bash
cd k8s
./deploy.sh
```

**Get connection information:**
```bash
./connect.sh
```

**Example output:**
```
Kafka broker: 138.197.252.214:9092
kafka-ui (use port-forward):
  kubectl port-forward -n kafka svc/kafka-ui 8080:8080
```

### Access Production Services

**Access kafka-ui:**
```bash
kubectl port-forward -n kafka svc/kafka-ui 8080:8080
# Open http://localhost:8080
```

**Run Producer against Production:**
```bash
cd producer
KAFKA_BROKER=<EXTERNAL-IP>:9092 cargo run
```

**View Production Logs:**
```bash
kubectl logs -n kafka -l app=kafka-broker --tail=100 -f
kubectl logs -n kafka -l app=kafka-ui --tail=100 -f
```

**Check Status:**
```bash
kubectl get all -n kafka
kubectl get pods -n kafka
kubectl get svc -n kafka
```

### Production Cleanup

```bash
cd k8s
./cleanup.sh
```

## Switching Between Environments

**Switch to local (minikube):**
```bash
kubectl config use-context minikube
./k8s/connect.sh
```

**Switch to production (DigitalOcean):**
```bash
kubectl config use-context do-<region>-<cluster-name>
./k8s/connect.sh
```

**Check current context:**
```bash
kubectl config current-context
```

## Producer Development

### Build and Run

```bash
cd producer
cargo build
cargo run
```

### Run with custom broker:
```bash
KAFKA_BROKER=localhost:9092 cargo run
```

### Message Format

Messages are serialized using Protocol Buffers with ZSTD compression:

```rust
UserEvent {
    event_id: UUID,
    timestamp: i64,
    user_id: String,
    event_type: EventType,
    data: UserEventData {
        email: Option<String>,
        username: Option<String>,
        metadata: HashMap<String, String>
    }
}
```

**Protobuf schema:** `schemas/proto/user_event.proto` (central schema management)

### Viewing Messages in kafka-ui

1. Port-forward kafka-ui
2. Navigate to Topics → `user.events`
3. Click "Messages" tab
4. Messages are automatically deserialized from protobuf

## Project Structure

```
hermes/
├── docker-compose.yaml          # Local development with Docker
├── schemas/                     # Central schema definitions
│   ├── proto/
│   │   └── user_event.proto    # Protobuf schema (source of truth)
│   ├── Makefile                # Validate, compile, and sync schemas
│   └── README.md               # Schema management guide
├── producer/                    # Rust Kafka producer
│   ├── src/
│   │   ├── main.rs             # Producer implementation
│   │   └── bin/
│   │       └── consumer.rs     # Optional consumer for debugging
│   ├── build.rs                # Protobuf code generation
│   ├── Cargo.toml              # Rust dependencies
│   └── README.md               # Producer documentation
└── k8s/                        # Kubernetes manifests
    ├── deploy.sh               # Unified deployment script
    ├── cleanup.sh              # Cleanup script
    ├── connect.sh              # Get connection info
    ├── namespace.yaml          # Kafka namespace
    ├── kafka-broker.yaml       # Single broker (current)
    ├── kafka-broker-3node.yaml # 3-broker HA setup
    ├── kafka-ui.yaml           # kafka-ui deployment
    ├── protobuf-configmap.yaml # Protobuf schemas (generated from schemas/)
    ├── README.md               # K8s deployment docs
    ├── DIGITALOCEAN.md         # DigitalOcean specific guide
    └── PRODUCTION-SETUP.md     # Production best practices
```

## Configuration

### Environment Variables

**Producer:**
- `KAFKA_BROKER` - Broker address (default: `localhost:9092`)

**Kafka Broker:**
- Configured via k8s manifests or docker-compose

### Resource Limits (Kubernetes)

**Current (Single Broker):**
- Memory: 2Gi request, 4Gi limit
- CPU: 1 core request, 2 cores limit
- Storage: 20Gi persistent

**Cluster Resources:**
- 5 nodes × (4 CPU, 8GB RAM)
- Total capacity: 20 CPUs, 40GB RAM

## Common Tasks

### View All Messages in a Topic

Using kafka-ui:
1. Port-forward: `kubectl port-forward -n kafka svc/kafka-ui 8080:8080`
2. Open http://localhost:8080
3. Topics → user.events → Messages

### Delete All Messages in a Topic

```bash
kubectl exec -it -n kafka kafka-broker-0 -- /bin/bash
kafka-topics --bootstrap-server localhost:9092 --delete --topic user.events
```

### Create a New Topic

```bash
kubectl exec -it -n kafka kafka-broker-0 -- /bin/bash
kafka-topics --bootstrap-server localhost:9092 --create --topic new-topic --partitions 3 --replication-factor 1
```

### List All Topics

```bash
kubectl exec -it -n kafka kafka-broker-0 -- /bin/bash
kafka-topics --bootstrap-server localhost:9092 --list
```

### Check Consumer Group Status

```bash
kubectl exec -it -n kafka kafka-broker-0 -- /bin/bash
kafka-consumer-groups --bootstrap-server localhost:9092 --list
kafka-consumer-groups --bootstrap-server localhost:9092 --describe --group <group-name>
```

## Troubleshooting

### Producer can't connect

**Check broker is running:**
```bash
kubectl get pods -n kafka
```

**Check broker logs:**
```bash
kubectl logs -n kafka kafka-broker-0 --tail=100
```

**Verify connection:**
```bash
# Local
telnet localhost 9092

# Production
telnet <EXTERNAL-IP> 9092
```

### kafka-ui shows infinite loading

**Restart kafka-ui:**
```bash
kubectl rollout restart deployment/kafka-ui -n kafka
```

**Check kafka-ui logs:**
```bash
kubectl logs -n kafka -l app=kafka-ui --tail=50
```

### Broker won't start

**Check persistent volume:**
```bash
kubectl get pvc -n kafka
```

**Delete and recreate (DATA LOSS):**
```bash
kubectl delete statefulset kafka-broker -n kafka
kubectl delete pvc kafka-data-kafka-broker-0 -n kafka
kubectl apply -f k8s/kafka-broker.yaml
```

### Check cluster context

```bash
kubectl config current-context
kubectl config get-contexts
```

## Costs (DigitalOcean Production)

**Current Setup:**
- LoadBalancer (1): $12/month
- Block Storage (20GB): $2/month
- **Total: $14/month** (+ existing node costs)

**With Reserved IP (Recommended for Production):**
- Reserved IP: $4/month
- LoadBalancer: $12/month
- Block Storage: $20GB × 1 = $2/month
- **Total: $18/month**

**3-Broker HA Setup:**
- Reserved IP: $4/month
- LoadBalancer: $12/month
- Block Storage: 20GB × 3 = $6/month
- **Total: $22/month**

## Schema Management

Protobuf schemas are managed centrally in the `schemas/` directory. This ensures type safety and a single source of truth:

```bash
cd schemas
vim proto/user_event.proto  # Edit schema
make validate               # Check syntax
make sync                   # Update k8s ConfigMap
cd ../producer
cargo build                 # Rebuild with new schema
```

See [Schema Management Guide](schemas/README.md) for complete workflow and best practices.

## Further Reading

- [Schema Management Guide](schemas/README.md) - **Start here for schema changes**
- [Kubernetes Deployment Guide](k8s/README.md)
- [DigitalOcean Setup](k8s/DIGITALOCEAN.md)
- [Production Best Practices](k8s/PRODUCTION-SETUP.md)
- [Producer Documentation](producer/README.md)

## Support

For issues or questions:
1. Check logs: `kubectl logs -n kafka <pod-name>`
2. Check status: `kubectl get all -n kafka`
3. Review troubleshooting section above
