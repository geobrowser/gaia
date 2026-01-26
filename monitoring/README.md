# Kubernetes Monitoring Stack

Prometheus + Grafana for cluster-wide monitoring.

## Components

- **Prometheus** - Scrapes and stores metrics
- **Grafana** - Visualization dashboards
- **Alertmanager** - Alert routing
- **node-exporter** - Host metrics (CPU, memory, disk per node)
- **kube-state-metrics** - Kubernetes object state

## Deploy

```bash
# Create namespace first
kubectl apply -f monitoring/k8s/namespace.yaml

# Apply the stack (CRDs must be applied first, may need to run twice)
kubectl apply -f monitoring/k8s/prometheus-stack.yaml --server-side

# If you get errors about CRDs not existing, wait a moment and re-run
kubectl apply -f monitoring/k8s/prometheus-stack.yaml --server-side
```

## Access Grafana

```bash
# Port forward to access locally
kubectl port-forward svc/kube-prometheus-stack-grafana 3000:80 -n monitoring

# Open http://localhost:3000
```

**Credentials:**
- Username: `admin`
- Password: Run this to retrieve:
  ```bash
  kubectl get secret kube-prometheus-stack-grafana -n monitoring \
    -o jsonpath="{.data.admin-password}" | base64 -d && echo
  ```

## Access Prometheus UI

```bash
kubectl port-forward svc/kube-prometheus-stack-prometheus 9090:9090 -n monitoring
# Open http://localhost:9090
```

## Updating

To update the stack, modify `values.yaml` and regenerate:

```bash
helm template kube-prometheus-stack prometheus-community/kube-prometheus-stack \
  --namespace monitoring \
  --include-crds \
  -f monitoring/values.yaml \
  > monitoring/k8s/prometheus-stack.yaml

kubectl apply -f monitoring/k8s/prometheus-stack.yaml --server-side
```

## Values

See `values.yaml` for configuration options:
- Grafana ingress (currently disabled)
- Prometheus retention (10 days)
- Resource limits
- Persistent storage (currently using emptyDir)

## What Gets Monitored

Out of the box:
- Node CPU, memory, disk, network
- Pod resource usage
- Container restarts
- Kubernetes API server
- Deployment/StatefulSet status

Pre-built Grafana dashboards are included for all of the above.
