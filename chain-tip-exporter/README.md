# Chain Tip Exporter

Publishes Prometheus gauges for the latest Geo chain block via a plain EVM JSON-RPC endpoint. Do not use the Substreams endpoint here; it is a gRPC Substreams API, not an EVM JSON-RPC API.

## Required Kubernetes Secret

The exporter runs in the `monitoring` namespace. Ensure `regcred` exists there so Kubernetes can pull from the private DigitalOcean registry.

Create one secret per environment before deploying:

```bash
kubectl create secret generic chain-tip-exporter-production-secrets \
  -n monitoring \
  --from-literal=RPC_URL='<geo-json-rpc-url>'

kubectl create secret generic chain-tip-exporter-staging-secrets \
  -n monitoring \
  --from-literal=RPC_URL='<geo-json-rpc-url>'
```

`RPC_URL` must support `eth_chainId`, `eth_blockNumber`, and `eth_getBlockByNumber`. For the current Hermes deployment, use the Geo testnet RPC (`chainId` `19411`) so the chain tip matches the Substreams data Hermes consumes.

`LATEST_BLOCK_BEHIND_THRESHOLD_SECS` is set in the k8s manifests to `300` seconds. When the latest block timestamp is older than this threshold, the exporter emits an error-level log; with `SENTRY_DSN` configured, that becomes a Sentry issue.

Optional keys in the same secret:

- `SENTRY_DSN`
- `SENTRY_RELEASE`

## Kubernetes Manifests

Deploy the exporter and scrape config:

```bash
kubectl apply -f monitoring/k8s/namespace.yaml
kubectl apply -f chain-tip-exporter/k8s/production/chain-tip-exporter.yaml
kubectl apply -f chain-tip-exporter/k8s/staging/chain-tip-exporter.yaml
kubectl apply -f monitoring/k8s/hermes-metrics-servicemonitor.yaml
kubectl apply -f monitoring/k8s/hermes-lag-dashboard.yaml
```

If older exporter Deployments exist in `knowledge` or `knowledge-staging`, delete them after the `monitoring` rollout is healthy.

