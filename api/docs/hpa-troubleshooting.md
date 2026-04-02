# API HPA Troubleshooting

Runbook for diagnosing and fixing HPA scaling issues in the API deployment.

## Quick Status Check

```bash
kubectl describe hpa api -n api
kubectl get pods -n api
```

**Healthy:** All metrics show numeric values, replicas between min (4) and max (8).
**Broken:** Any metric shows `<unknown>`, or `ScalingActive: False` in conditions.

## HPA Metrics Reference

| Metric | Type | Target | Source |
|---|---|---|---|
| `api_ingress_p95_latency_seconds_2m` | External | 1s | Recording rule → Prometheus adapter |
| `api_ingress_p99_latency_seconds_2m` | External | 2s | Recording rule → Prometheus adapter |
| `api_ready_replica_pressure` | External | 1 | kube-state-metrics → Prometheus adapter |
| `api_ingress_503_ratio_rate5m` | External | 0.015 | nginx metrics → Prometheus adapter |
| CPU | Resource | 70% | kubelet |
| Memory | Resource | 80% | kubelet |
| `gaia_api_graphql_pool_saturated` | Pod | 100m | API `/health/metrics` endpoint |

## Diagnosing `<unknown>` External Metrics

When external metrics show `<unknown>`, the HPA can't compute scale decisions and stays at its current replica count.

### 1. Check the metrics chain

```bash
# Port-forward to Prometheus
kubectl port-forward -n monitoring svc/kube-prometheus-stack-prometheus 9090:9090 &

# Check if the recording rules produce data
curl -s 'http://localhost:9090/api/v1/query?query=api:ingress_p95_latency_seconds:2m'
curl -s 'http://localhost:9090/api/v1/query?query=api:ingress_p99_latency_seconds:2m'

# Check if the source histogram exists
curl -s 'http://localhost:9090/api/v1/query?query=count(nginx_ingress_controller_request_duration_seconds_bucket%7Bhost%3D%22testnet-api.geobrowser.io%22%7D)'

# Check what hosts the ingress controller reports
curl -s 'http://localhost:9090/api/v1/query?query=count(nginx_ingress_controller_requests)+by+(host)'

# Check kube-state-metrics for replica pressure
curl -s 'http://localhost:9090/api/v1/query?query=kube_deployment_status_replicas_available%7Bnamespace%3D%22api%22%2Cdeployment%3D%22api%22%7D'
```

### 2. Check if recording rules are loaded

```bash
# List all recording rules with "ingress" in the name
curl -s 'http://localhost:9090/api/v1/rules?type=record' | python3 -c "
import json, sys
for g in json.load(sys.stdin)['data']['groups']:
    for r in g['rules']:
        if 'ingress' in r['name']:
            print(f\"{r['name']}: health={r['health']}\")
"
```

If the 2m rules are missing, re-apply the PrometheusRule:

```bash
kubectl apply -f monitoring/k8s/api-ingress-rules.yaml
```

### 3. Check the Prometheus adapter

```bash
# List available external metrics
kubectl get --raw "/apis/external.metrics.k8s.io/v1beta1"

# If metrics exist in Prometheus but not in the adapter, restart it
kubectl rollout restart deployment prometheus-adapter -n monitoring
```

### 4. Verify the adapter config is applied

```bash
kubectl get configmap adapter-config -n monitoring -o yaml | grep "2m"
```

If missing, re-apply:

```bash
kubectl apply -f monitoring/k8s/prometheus-adapter-config.yaml
kubectl rollout restart deployment prometheus-adapter -n monitoring
```

## Diagnosing OOM Kills

```bash
# Check for OOMKilled pods
kubectl describe pods -n api -l app=api | grep -A5 "Last State"

# Check current memory usage
kubectl top pods -n api
```

### Memory configuration

| Setting | Staging | Production |
|---|---|---|
| Memory request | 512Mi | 768Mi |
| Memory limit | 1Gi | 2Gi |
| HPA memory target | — | 80% of request (614Mi) |

If OOM kills recur after limit increase, investigate:
- Large GraphQL responses (check PostGraphile query complexity)
- Connection pool leaks (check `/health/detailed` for pool stats)
- Memory spikes during schema builds (PostGraphile startup)

## Relevant Files

| File | Purpose |
|---|---|
| `api/k8s/production/api.yaml` | Deployment (resource limits) |
| `api/k8s/production/hpa.yaml` | HPA scaling config |
| `monitoring/k8s/api-ingress-rules.yaml` | Prometheus recording rules |
| `monitoring/k8s/prometheus-adapter-config.yaml` | Adapter external metric queries |
| `monitoring/k8s/api-capacity-alerts.yaml` | Alerting rules |
| `api/src/services/dbSaturation.ts` | Pool saturation state machine |
| `api/src/health.ts` | Health/metrics endpoints |
