# Monitoring — new testnet cluster (`do-nyc2-geo-testnet-k8s`)

The v2 stack runs in a single `gaia` namespace on a cluster that serves traffic
through a **Cilium Gateway**, not ingress-nginx. The manifests in the parent
directory assume the old cluster's namespace layout (`knowledge`, `api`,
`api-staging`) and its nginx ingress, so the files here replace the ones that
do not carry over.

## Deploy

The chart render and the cluster-agnostic rules come from the parent directory:

```bash
CTX=do-nyc2-geo-testnet-k8s

kubectl --context $CTX apply -f monitoring/k8s/namespace.yaml
kubectl --context $CTX apply -f monitoring/k8s/prometheus-stack.yaml --server-side   # run twice; CRDs first
kubectl --context $CTX apply -f monitoring/k8s/hermes-lag-alerts.yaml                # cluster-agnostic
```

Two secrets must exist in `monitoring` before Alertmanager and Grafana start.
Both are copied from the old cluster; neither is in git:

```bash
# Slack webhooks (keys: url, url-staging) — Alertmanager mounts this via
# alertmanagerSpec.secrets, and fails to send without it.
kubectl --context $CTX -n monitoring create secret generic alertmanager-slack-webhook ...

# Grafana admin (keys: admin-user, admin-password) — values.yaml sets
# admin.existingSecret, so Grafana CrashLoops with CreateContainerConfigError
# if this is absent.
kubectl --context $CTX -n monitoring create secret generic kube-prometheus-stack-grafana ...

# Registry pull secret, for chain-tip-exporter.
kubectl --context $CTX -n gaia get secret regcred -o yaml \
  | sed 's/namespace: gaia/namespace: monitoring/' | kubectl --context $CTX apply -f -

# RPC endpoint for chain-tip-exporter, reusing the executor's chain-55516 URL.
kubectl --context $CTX -n monitoring create secret generic chain-tip-exporter-secrets \
  --from-literal=RPC_URL="$(kubectl --context $CTX -n gaia get secret \
      proposal-executor-credentials -o jsonpath='{.data.RPC_URL}' | base64 -d)"
```

Then this directory:

```bash
kubectl --context $CTX apply -f monitoring/k8s/v2/
```

## What is here and why

| file | why it differs from the parent copy |
|---|---|
| `hermes-metrics-servicemonitor.yaml` | scrapes `gaia` instead of `knowledge` / `knowledge-staging` |
| `api-capacity-alerts.yaml` | targets `gaia`; drops the `api-staging` duplicate; latency/5xx alerts rebased onto the Gateway |
| `gateway-metrics.yaml` | scrapes Cilium's Envoy and provides the `api:gateway_*` recording rules that replace the nginx-derived `api:ingress_*` pair |
| `chain-tip-exporter.yaml` | this cluster had no exporter at all, so `HermesBehindChainTip` could never fire |

## Deliberately not ported

- **`ingress-nginx-metrics.yaml`, `api-ingress-rules.yaml`, `api-ingress-dashboard.yaml`** —
  there is no ingress-nginx here. `gateway-metrics.yaml` supplies the equivalent
  signals from Envoy.
- **`prometheus-adapter-*.yaml`** — the API's HPA runs on cpu/memory via
  metrics-server. Nothing on this cluster consumes external metrics.
- **`kafka-exporter.yaml`, `opensearch-exporter.yaml`** and their lag alerts —
  these need this cluster's Kafka and OpenSearch credentials wiring first. Until
  they land, `KafkaConsumerLagHigh` / `KafkaConsumerStuck` are **not** watching
  the v2 pipeline.
- **Dashboards** (`*-dashboard.yaml`) — the `gaia-v2-*` ones point at the old
  cluster's `gaia-v2` namespace and need the same re-pointing treatment.

## Alert routing

`values.yaml` routes `namespace =~ ".*-staging"` to `#infra-alerts-staging` and
everything else to `#alerts`. This cluster's namespace is `gaia`, which matches
neither staging pattern, so **v2 alerts go to `#alerts`** — correct now that v2
serves production, but note the old cluster's Alertmanager is still running and
also posting there.
