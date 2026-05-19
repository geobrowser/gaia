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

# Scrape ingress-nginx request metrics + API ingress recording rules
kubectl apply -f monitoring/k8s/ingress-nginx-metrics.yaml
kubectl apply -f monitoring/k8s/api-ingress-rules.yaml
kubectl apply -f monitoring/k8s/api-ingress-dashboard.yaml
```

## Access Grafana

```bash
# Port forward to access locally
kubectl port-forward svc/kube-prometheus-stack-grafana 3000:80 -n monitoring

# Open http://localhost:3000
```

**Credentials:**
- Username: `admin`
- Password lives in the `kube-prometheus-stack-grafana` Secret (managed manually — the chart no longer renders it, see `values.yaml` `grafana.admin.existingSecret`).

Retrieve:
```bash
kubectl -n monitoring get secret kube-prometheus-stack-grafana \
  -o jsonpath="{.data.admin-password}" | base64 -d && echo
```

Create on a fresh cluster (Grafana pod CrashLoops until this exists):
```bash
PW=$(openssl rand -base64 24)
kubectl -n monitoring create secret generic kube-prometheus-stack-grafana \
  --from-literal=admin-user=admin \
  --from-literal=admin-password="$PW"
echo "save to password manager: $PW"
```

Rotate:
```bash
PW=$(openssl rand -base64 24)
kubectl -n monitoring patch secret kube-prometheus-stack-grafana --type=json \
  -p="[{\"op\":\"replace\",\"path\":\"/data/admin-password\",\"value\":\"$(printf %s "$PW" | base64)\"}]"
kubectl -n monitoring rollout restart deployment/kube-prometheus-stack-grafana
echo "save to password manager: $PW"
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
  --version 81.2.2 \
  --namespace monitoring \
  --include-crds \
  -f monitoring/values.yaml \
  > monitoring/k8s/prometheus-stack.yaml

kubectl apply -f monitoring/k8s/prometheus-stack.yaml --server-side
```

Last rendered with helm `v4.2.0` and chart `kube-prometheus-stack@81.2.2`; use the same versions to keep diffs minimal.

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
