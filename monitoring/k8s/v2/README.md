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

# Registry pull secret: nothing to create. chain-tip-exporter uses `geo`, which DigitalOcean's
# registry integration (registry_enabled on the cluster) maintains in every namespace. Don't
# copy a hand-made `regcred` around: it is generated from one person's DO token and stops
# working when that person is removed (GEO-3010).

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
| `kg-indexer-alerts.yaml` | new: `KgIndexerBlockDropped` (GEO-2884's silent loss) and `KgIndexerMetricsMissing` |
| `indexer-alerts.yaml` | new: `IndexerMetricsMissing`, `IndexerDroppedData` and `IndexerRestartingRepeatedly` for the six indexers below |

## Deliberately not ported

- **`ingress-nginx-metrics.yaml`, `api-ingress-rules.yaml`, `api-ingress-dashboard.yaml`** —
  there is no ingress-nginx here. `gateway-metrics.yaml` supplies the equivalent
  signals from Envoy.
- **`prometheus-adapter-*.yaml`** — the API's HPA runs on cpu/memory via
  metrics-server. Nothing on this cluster consumes external metrics.
- **`opensearch-exporter.yaml`** — needs this cluster's OpenSearch credentials
  wired into `monitoring` first.

`kafka-exporter.yaml` and `kafka-consumer-lag-alerts.yaml` (in `monitoring/k8s/`,
not ported copies) have run on this cluster since 2026-09-24, against
`geo-testnet-kafka`. Their credentials secret, `monitoring/kafka-exporter-creds`,
is a copy of `gaia/kafka-credentials`; see the exporter's header.
- **Dashboards** (`*-dashboard.yaml`) — the `gaia-v2-*` ones point at the old
  cluster's `gaia-v2` namespace and need the same re-pointing treatment.

## What is scraped

Checked against the live cluster on 2026-09-30 (GEO-2959).

Scraped: `api`, `atlas`, `hermes-pipeline`, `hermes-ipfs-cache`,
`chain-tip-exporter`, `kafka-exporter`, `geo-chat-api`, `kg-indexer`.

All of the Kafka consumers and indexers now serve `/metrics` on 9464 through
`hermes_instrumentation::metrics::install` and are in
`hermes-metrics-servicemonitor.yaml`. Port 9464 is separate from the `/healthz`
server that `search-indexer`, `notification-indexer` and `delivery-worker` run
on 8080, so every service has the same `metrics` port and the ServiceMonitor
needs one endpoint. It also keeps scrapes off the health server:
`search-indexer` runs that on its own thread so probes survive a saturated
runtime, and installs the metrics listener the same way.

`kg-indexer` was first (GEO-2959, #988). It publishes
`hermes_latest_processed_block{,_timestamp}`, so `HermesBehindChainTip` covers
it, plus `kg_indexer_blocks_dropped_total{transient}`,
`kg_indexer_blocks_processed_total`, `kg_indexer_events_processed_total`,
`kg_indexer_batch_retries_total` and `kg_indexer_messages_unparseable_total`.
The gauge is not set from the blocks it writes: most blocks have no events for
it and arrive only as a summary, so that would stall in quiet stretches. It
follows the highest block summary read, empty or not, capped just below the
lowest block still buffered. hermes-pipeline emits one summary per chain block
(889 summaries against 889 tip blocks over 24h on 2026-09-30), so it tracks
the tip whenever kg-indexer is keeping up.

The others (also GEO-2959) publish counters only, plus one backlog gauge:

| service | series |
|---|---|
| `search-indexer` | `search_indexer_events_processed_total`, `_documents_indexed_total`, `_operations_total`, `_operations_failed_total`, `_operations_by_kind_total{kind}`, `_bulk_calls_total`, `_bulk_{wall,took}_milliseconds_total`, gauge `_canonical_graph_nodes`. These are the in-memory `SearchIndexerMetrics`, copied into the recorder every 10s |
| `vote-indexer` | `vote_indexer_votes_processed_total`, `_votes_dropped_total`, `_messages_rejected_total{reason}`, `_ranking_refresh_failures_total` |
| `topology-indexer` | `topology_indexer_diffs_applied_total`, `_diffs_dropped_total`, `_messages_unparseable_total` |
| `ranking-indexer` | `ranking_indexer_messages_processed_total{topic}`, `_messages_skipped_total{topic}`, `_transient_retries_total{topic}` |
| `notification-indexer` | `notification_indexer_events_processed_total{consumer}`, `_events_failed_total{consumer,reason}`, `_notifications_inserted_total`, `_poller_errors_total{poller}` |
| `delivery-worker` | `delivery_worker_deliveries_total{outcome}`, `_claim_errors_total`, `_stale_claims_reset_total`, gauges `_pending_deliveries`, `_in_progress_deliveries` |

None of them publishes a position gauge. Their topics only carry a message when
someone acts, so a "last processed" value would stall through quiet stretches
and look like lag; per-partition lag is already `kafka-exporter`'s job
(`kafka-consumer-lag-alerts.yaml`). `delivery-worker` reads Postgres, not
Kafka, and its pending-deliveries gauge is correct when idle (it reads 0).

Three failure paths lose data without stopping the consumer, and are what
`IndexerDroppedData` watches: a vote-indexer or topology-indexer batch whose
transaction fails, and a notification-indexer database error. Each leaves the
offset uncommitted to be "retried on restart", but the next success on the
partition commits past it. `search-indexer` (any NACKed batch) and
`ranking-indexer` (a transient error that outlasts its retries) exit instead,
so their failures show as restarts (`IndexerRestartingRepeatedly`).

## Dashboards

None of the repo's dashboards is installed. Only the stock
`kube-prometheus-stack-*` ones are. Against live metric names on 2026-09-30:

| file | state |
|---|---|
| `api-ingress-dashboard.yaml` | every query is an `api:ingress_*` recording rule or an nginx metric; none exists here |
| `gaia-overview-dashboard.yaml` | ingress, `elasticsearch_*` and namespace (`api`, `knowledge`, `search`, `scoring`) queries are all empty; the `gaia_api_*` and kube-state panels would work with the namespace re-pointed to `gaia` |
| `gaia-v2-overview-dashboard.yaml` | the same, for namespace `gaia-v2` and `gaia_v2:ingress_*` |
| `hermes-lag-dashboard.yaml` | the metrics exist, but it filters `gaia_namespace` on `knowledge` / `knowledge-staging`; live values are `gaia` or absent |
| `kafka-consumer-lag-dashboard.yaml` | current; every metric exists |

## Alert routing

`values.yaml` routes `namespace =~ ".*-staging"` to `#infra-alerts-staging` and
everything else to `#alerts`. This cluster's namespace is `gaia`, which matches
neither staging pattern, so **v2 alerts go to `#alerts`** — correct now that v2
serves production, but note the old cluster's Alertmanager is still running and
also posting there.
