# Kubernetes Deployment for Kafka Producer

This directory contains Kubernetes manifests to deploy the Kafka infrastructure. **The same manifests work for both local (minikube) and DigitalOcean** - only the kubectl context changes.

## Components

- **namespace.yaml** - Creates the `kafka` namespace
- **kafka-broker.yaml** - Kafka broker deployment and service
- **kafka-ui.yaml** - kafka-ui deployment and service for viewing messages
- **protobuf-configmap.yaml** - ConfigMap containing protobuf schemas
- **deploy.sh** - Unified deployment script (auto-detects environment)
- **cleanup.sh** - Cleanup script
- **connect.sh** - Shows connection information for your environment

## Quick Start

### 1. Choose Your Environment

**Local (minikube):**
```bash
minikube start
kubectl config use-context minikube
```

**DigitalOcean:**
```bash
# First time: Install doctl and authenticate
brew install doctl
doctl auth init

# Connect to your cluster
doctl kubernetes cluster kubeconfig save <cluster-name>
```

### 2. Deploy

The deployment is **identical** for both environments:

```bash
cd k8s
./deploy.sh
```

The script automatically detects your environment and provides the appropriate access instructions.

### 3. Get Connection Info

```bash
./connect.sh
```

This shows you how to connect based on your current environment:
- **Local**: Port-forward commands
- **DigitalOcean**: External LoadBalancer IPs

### 4. Run the Producer

**Local:**
```bash
# In one terminal, forward the port
kubectl port-forward -n kafka svc/broker 9092:9092

# In another terminal, run the producer
cd producer
KAFKA_BROKER=localhost:9092 cargo run
```

**DigitalOcean:**
```bash
# Get the external IP
./connect.sh

# Run the producer with the external IP
cd producer
KAFKA_BROKER=<EXTERNAL-IP>:9092 cargo run
```

## Switching Between Environments

You can easily switch between local and remote:

```bash
# Switch to local
kubectl config use-context minikube
./connect.sh

# Switch to DigitalOcean
kubectl config use-context do-<region>-<cluster-name>
./connect.sh
```

## Manual Deployment (Optional)

If you prefer to deploy manually:

```bash
kubectl apply -f namespace.yaml
sleep 2
kubectl apply -f protobuf-configmap.yaml
kubectl apply -f kafka-broker.yaml
kubectl apply -f kafka-ui.yaml
```

## Useful Commands

```bash
# Check deployment status
kubectl get all -n kafka

# View logs
kubectl logs -n kafka -l app=kafka-broker --tail=50 -f
kubectl logs -n kafka -l app=kafka-ui --tail=50 -f

# Check services
kubectl get svc -n kafka

# Get current context
kubectl config current-context
```

## Cleanup

```bash
./cleanup.sh
```

Or manually:
```bash
kubectl delete namespace kafka
```

## Environment Differences

| Feature | Local (minikube) | DigitalOcean |
|---------|------------------|--------------|
| **Deployment** | Same manifests | Same manifests |
| **Access** | Port-forward required | External LoadBalancer IPs |
| **Cost** | Free | ~$72/month (2 nodes + 2 LBs) |
| **DNS** | localhost | External IPs |
| **Persistence** | emptyDir (ephemeral) | Can use DO Block Storage |

## Production Considerations

For production deployments (local or remote), consider:

1. **Persistence**: Replace `emptyDir` with `PersistentVolumeClaim`
2. **Replication**: Increase Kafka replicas for HA
3. **Resources**: Set CPU/memory requests and limits
4. **Security**: Add NetworkPolicies and RBAC
5. **Monitoring**: Add Prometheus/Grafana
6. **Backups**: Configure volume snapshots

See [DIGITALOCEAN.md](./DIGITALOCEAN.md) for detailed DigitalOcean-specific information.
