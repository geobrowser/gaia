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

- **`ingress-nginx-metrics.yaml`, `api-ingress-rules.yaml`** — there is no
  ingress-nginx here. `gateway-metrics.yaml` supplies the equivalent signals
  from Envoy. (`api-ingress-dashboard.yaml` was deleted: nothing it queried
  exists on this cluster.)
- **`prometheus-adapter-*.yaml`** — the API's HPA runs on cpu/memory via
  metrics-server. Nothing on this cluster consumes external metrics.
- **`opensearch-exporter.yaml`** — needs this cluster's OpenSearch credentials
  wired into `monitoring` first.

`kafka-exporter.yaml` and `kafka-consumer-lag-alerts.yaml` (in `monitoring/k8s/`,
not ported copies) have run on this cluster since 2026-09-24, against
`geo-testnet-kafka`. Their credentials secret, `monitoring/kafka-exporter-creds`,
is a copy of `gaia/kafka-credentials`; see the exporter's header.

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

The dashboards are ConfigMaps labelled `grafana_dashboard: "1"`. Grafana's
dashboard sidecar watches every namespace for that label (`LABEL` /
`NAMESPACE=ALL` in `prometheus-stack.yaml`) and loads them without a restart.
**None is installed yet** — on 2026-09-30 the only labelled ConfigMaps on the
cluster were the stock `kube-prometheus-stack-*` ones.

| file | uid | what it shows |
|---|---|---|
| `gaia-overview-dashboard.yaml` | `gaia-overview` | API traffic, latency and error codes from the Cilium Gateway (Envoy), DB/GraphQL pool health, per-service resource % of limit, pod health, GraphQL cost and response size, and an **Indexers** row: throughput, dropped/failed data, retries and rejects, delivery queue, search-indexer bulk latency, scrape `up` |
| `hermes-lag-dashboard.yaml` | `hermes-lag` | chain tip against `hermes_latest_processed_block` for hermes-pipeline, hermes-ipfs-cache, atlas and kg-indexer |
| `kafka-consumer-lag-dashboard.yaml` | `kafka-consumer-lag` | per-group, per-topic consumer lag from `kafka-exporter` |

Every query in all three returned data from this cluster's Prometheus on
2026-09-30 (114 queries, none empty). They live in the parent directory, but
target this cluster only: `namespace="gaia"`, the Gateway's
`envoy_cluster_name=~".*gaia_api_3000"`, and chain-tip's `gaia_namespace="gaia"`.

Install (server-side apply: the overview is ~100 KB, which is close to the
256 KB limit on client-side apply's last-applied annotation):

```bash
CTX=do-nyc2-geo-testnet-k8s
kubectl --context $CTX apply --server-side -f monitoring/k8s/gaia-overview-dashboard.yaml
kubectl --context $CTX apply --server-side -f monitoring/k8s/hermes-lag-dashboard.yaml
kubectl --context $CTX apply --server-side -f monitoring/k8s/kafka-consumer-lag-dashboard.yaml

# confirm the sidecar picked them up
kubectl --context $CTX -n monitoring logs deploy/kube-prometheus-stack-grafana -c grafana-sc-dashboard --tail=20
```

Notes on what changed from the old-cluster versions:

- **API traffic** comes from Envoy, not nginx. Latency is the upstream leg
  only (`envoy_cluster_upstream_rq_time`, milliseconds divided to seconds), so
  it reads a little below the old client-facing figure. Envoy has no 499; the
  error panel shows 500, 503 and upstream timeouts instead.
- **api CPU %** is against its *request*: the api container has a memory
  limit but no CPU limit, so a %-of-limit query would be empty.
- **The OpenSearch row is gone.** Its `elasticsearch_*` series need
  `opensearch-exporter.yaml`, which is not deployed here (see above). The
  panels are in git history if the exporter is ever installed.
- `gaia-v2-overview-dashboard.yaml` (namespace `gaia-v2` on the deleted
  cluster) and `api-ingress-dashboard.yaml` (nginx only) were deleted.

`search-indexer-deploy/grafana/dashboards/opensearch-overview-dashboard.json`
is not a cluster dashboard: it is provisioned by that directory's local
`docker-compose.yaml`, which runs its own `elasticsearch-exporter`, so its
`elasticsearch_*` queries do have data there.

## Alert routing

`values.yaml` routes `namespace =~ ".*-staging"` to `#infra-alerts-staging` and
everything else to `#alerts`. This cluster's namespace is `gaia`, which matches
neither staging pattern, so **v2 alerts go to `#alerts`** — correct now that v2
serves production, but note the old cluster's Alertmanager is still running and
also posting there.
