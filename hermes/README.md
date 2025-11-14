# Kubernetes Deployment for Kafka Producer

This directory contains Kubernetes manifests to deploy the Kafka infrastructure. **Uses Kustomize overlays to support both local (minikube) and DigitalOcean** - only the kubectl context changes.

## Structure

```
hermes/
├── base/                    # Shared configuration
│   ├── kustomization.yaml
│   ├── namespace.yaml
│   ├── kafka-broker.yaml
│   ├── kafka-ui.yaml
│   └── protobuf-configmap.yaml
├── overlays/
│   ├── local/               # Minikube-specific config
│   │   └── kustomization.yaml
│   └── digitalocean/        # DigitalOcean-specific config
│       └── kustomization.yaml
├── deploy.sh                # Auto-detects environment
├── cleanup.sh
├── connect.sh
└── port-forward-local.sh    # Port-forward broker + UI (local only)
```

## Components

- **base/** - Shared Kubernetes manifests (namespace, broker, ui, configmap)
- **overlays/local/** - Local overrides (ClusterIP service)
- **overlays/digitalocean/** - Production overrides (LoadBalancer, external listener, DO storage)
- **deploy.sh** - Unified deployment script (auto-detects environment and applies correct overlay)
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
# In one terminal, start port-forwards
./port-forward-local.sh

# In another terminal, run the producer
cd ../hermes-producer
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

If you prefer to deploy manually with Kustomize:

```bash
# Local (minikube)
kubectl apply -k overlays/local

# DigitalOcean
kubectl apply -k overlays/digitalocean

# Or just the base (not recommended)
kubectl apply -k base
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
| **Deployment** | `overlays/local` | `overlays/digitalocean` |
| **Service Type** | ClusterIP | LoadBalancer |
| **Storage Class** | standard | do-block-storage |
| **Kafka Listener** | Internal only | Internal + External (LoadBalancer IP) |
| **Access** | Port-forward required | External LoadBalancer IPs |
| **Cost** | Free | ~$72/month (2 nodes + 2 LBs) |

## Production Considerations

For production deployments (local or remote), consider:

1. **Persistence**: Replace `emptyDir` with `PersistentVolumeClaim`
2. **Replication**: Increase Kafka replicas for HA
3. **Resources**: Set CPU/memory requests and limits
4. **Security**: Add NetworkPolicies and RBAC
5. **Monitoring**: Add Prometheus/Grafana
6. **Backups**: Configure volume snapshots

See [DIGITALOCEAN.md](./DIGITALOCEAN.md) for detailed DigitalOcean-specific information.
