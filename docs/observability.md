# Observability

Reference document mapping the monitoring and observability landscape across Gaia's services and Kubernetes infrastructure.

**Platform:** DigitalOcean Managed Kubernetes · **Last verified:** 2026-03-10

## 1. Overview

The observability stack consists of:

| Component | Purpose |
|-----------|---------|
| **Prometheus** (kube-prometheus-stack) | Metrics collection and storage (cluster-wide, `monitoring` namespace) |
| **Prometheus** (standalone) | OpenSearch metrics (search-namespace, `search` namespace) |
| **Grafana** (kube-prometheus-stack) | Dashboards for cluster-wide metrics (`monitoring` namespace) |
| **Grafana** (standalone) | OpenSearch dashboards (`search` namespace) |
| **Alertmanager** | Alert routing to Slack `#alerts` channel |
| **Sentry** | Error tracking and distributed tracing |
| **Axiom** | 100% trace retention via OTLP export (Sentry applies server-side sampling) |
| **node-exporter** | Host-level metrics on every node |
| **kube-state-metrics** | Kubernetes object state metrics |

### Namespace Topology

| Namespace | Services | Monitoring Stack |
|-----------|----------|-----------------|
| `api` | api | kube-prometheus-stack (recording rules, alerts, HPA custom metrics) |
| `knowledge` | atlas, hermes-pipeline, kg-indexer, hermes-ipfs-cache, proposal-executor | kube-prometheus-stack (alerts for atlas) |
| `knowledge-staging` | atlas (staging) | kube-prometheus-stack (alerts, lower severity) |
| `search` | search-indexer, opensearch-exporter | Standalone Prometheus + Grafana |
| `scoring` | scoring-service (CronJob), vote-indexer | — |
| `kafka` | kafka-ui | — |
| `monitoring` | Prometheus, Grafana, Alertmanager, node-exporter, kube-state-metrics | Self-monitoring |
| `ingress-nginx` | ingress-nginx-controller | ServiceMonitor scraped by kube-prometheus-stack |

**Config:** [`monitoring/values.yaml`](monitoring/values.yaml), [`monitoring/README.md`](monitoring/README.md)

## 2. Metrics & Dashboards

### Cluster-Wide Stack (kube-prometheus-stack)

- **Prometheus**: 10-day retention, emptyDir storage
  > ⚠️ **Data lost on pod restart.** Do not restart the Prometheus pod unless you accept losing all stored metrics.
- **Grafana**: Dashboards provisioned via ConfigMap sidecar — any ConfigMap with label `grafana_dashboard: "1"` is auto-discovered
- **node-exporter**: Host CPU, memory, disk, network on every node
- **kube-state-metrics**: K8s object state (deployments, pods, HPA status)

### Search-Namespace Stack

- **Prometheus**: 15-day retention, emptyDir storage, scrapes `opensearch-exporter:9114` at 15s intervals
- **Grafana**: Dashboards loaded via explicit volume mount from `grafana-dashboards` ConfigMap (not sidecar)
- **OpenSearch Exporter**: `quay.io/prometheuscommunity/elasticsearch-exporter:v1.7.0`, exports index/shard/cluster metrics

### API Ingress Recording Rules

[`monitoring/k8s/api-ingress-rules.yaml`](monitoring/k8s/api-ingress-rules.yaml) — PrometheusRule `api-ingress-observability` in `monitoring` namespace. 16 recording rules computed over `nginx_ingress_controller_requests{host="testnet-api.geobrowser.io"}`:

- **Request rates:** total, by status code, by HTTP method, error-class (4xx/5xx, 499, 500, 503)
- **Latency percentiles:** p50, p75, p95, p99 (overall), p99 by method
- **Error ratios:** 5xx, 499, 503 (`api:ingress_503_ratio:rate5m` also used by HPA)

### Grafana Dashboards

| Dashboard | Panels | Provisioned By | Config |
|-----------|--------|---------------|--------|
| API Ingress Observability | 9 panels | ConfigMap sidecar | [`monitoring/k8s/api-ingress-dashboard.yaml`](monitoring/k8s/api-ingress-dashboard.yaml) |
| Atlas Overview (Production) | 6 panels | ConfigMap sidecar | [`hermes/k8s/production/atlas-monitoring.yaml`](hermes/k8s/production/atlas-monitoring.yaml) |
| Atlas Overview (Staging) | 6 panels | ConfigMap sidecar | [`hermes/k8s/staging/atlas-monitoring.yaml`](hermes/k8s/staging/atlas-monitoring.yaml) |
| OpenSearch Overview | 20+ panels | Volume mount (kustomize) | [`search-indexer-deploy/grafana/dashboards/opensearch-overview-dashboard.json`](search-indexer-deploy/grafana/dashboards/opensearch-overview-dashboard.json) |

**Config:** [`monitoring/k8s/ingress-nginx-metrics.yaml`](monitoring/k8s/ingress-nginx-metrics.yaml) (ServiceMonitor for ingress-nginx)

## 3. Alerting

### Alert Channels

| Source | Channel | What |
|--------|---------|------|
| Alertmanager | Slack `#alerts` | Prometheus alert rules (API capacity, atlas health) |
| Sentry | Slack + Email | Application errors from all services with SENTRY_DSN (see §4 tracing table) |

### Alertmanager Configuration

- **Routing:** Group by `[namespace, alertname]`, 30s group wait, 5m group interval, 12h repeat
- **Suppressed:** `Watchdog` and `InfoInhibitor` alerts routed to null receiver
- **Inhibition rules:**
  - `critical` suppresses `warning` and `info` for the same namespace + alertname
  - `warning` suppresses `info` for the same namespace + alertname

**Config:** [`monitoring/values.yaml`](monitoring/values.yaml) (alertmanager section)

### Alert Naming Convention

`{Service}{Condition}` — e.g., `AtlasDeploymentUnavailable`, `ApiHpaMaxedWithHighP99`

### API Alerts

| Alert | Severity | Condition | First Check |
|-------|----------|-----------|-------------|
| `ApiReadinessDegraded` | warning | <75% of running pods ready for 5m | `kubectl get pods -n api` — check for unready pods |
| `ApiHpaMaxedWithHighP99` | critical | HPA at max replicas AND p99 > 2s for 10m | Check HPA status and latency dashboard |
| `Api503RateHigh` | warning | 503 ratio > 2% for 10m | Check API Ingress dashboard failure-class panel |

**Config:** [`monitoring/k8s/api-capacity-alerts.yaml`](monitoring/k8s/api-capacity-alerts.yaml)

### Atlas Alerts (Production)

| Alert | Severity | Condition | First Check |
|-------|----------|-----------|-------------|
| `AtlasDeploymentUnavailable` | critical | 0 available replicas for 5m | `kubectl get deploy atlas -n knowledge` |
| `AtlasCrashLoopBackOff` | critical | CrashLoopBackOff for 10m | `kubectl logs -n knowledge -l app=atlas --previous` |
| `AtlasRestartSpike` | warning | >3 restarts in 15m (pending 5m) | `kubectl get pods -n knowledge -l app=atlas` — check restart count |
| `AtlasMemoryHigh` | warning | Memory >90% of limit for 15m | Atlas Overview dashboard, memory panel |
| `AtlasCpuThrottlingHigh` | warning | CPU throttling >25% for 15m | Atlas Overview dashboard, throttling panel |

**Config:** [`hermes/k8s/production/atlas-monitoring.yaml`](hermes/k8s/production/atlas-monitoring.yaml)

### Atlas Alerts (Staging)

Same 5 alerts as production with identical thresholds. Only difference: `AtlasCrashLoopBackOff` is `warning` (not `critical`).

**Config:** [`hermes/k8s/staging/atlas-monitoring.yaml`](hermes/k8s/staging/atlas-monitoring.yaml)

## 4. Tracing

### 4a. Rust Services (hermes-instrumentation)

The [`hermes-instrumentation`](hermes-instrumentation/) crate provides unified telemetry for all Rust Hermes services. It wraps the `tracing` crate and provides:

- **Automatic span namespace prefixing** — e.g., a span named `fetch_content` in the `ipfs-cache` service appears as `ipfs-cache.fetch_content`
- **Two backends:**
  - `Console` — formatted stdout output (default, for development)
  - `Sentry` — OpenTelemetry spans exported to Sentry, with optional Axiom OTLP dual-export
- **Sentry integration:** ERROR events create Sentry issues; WARN/INFO become breadcrumbs and logs; DEBUG/TRACE are ignored
- **Axiom OTLP export:** When `AXIOM_TOKEN` and `AXIOM_DATASET` env vars are set, traces are batch-exported to Axiom at 100% (no sampling). Uses a blocking HTTP client on a dedicated thread.
- **Debug mode:** Set `SENTRY_DEBUG=true` to mirror spans to stdout alongside Sentry export

**Gotcha:** Initialize telemetry BEFORE creating the tokio runtime. The global tracing subscriber can only be set once.

**Services using hermes-instrumentation:** atlas, hermes-pipeline, kg-indexer, hermes-ipfs-cache, search-indexer, vote-indexer, scoring-service

**Config:** [`hermes-instrumentation/src/lib.rs`](hermes-instrumentation/src/lib.rs), [`hermes-instrumentation/src/config.rs`](hermes-instrumentation/src/config.rs), [`hermes-instrumentation/src/init.rs`](hermes-instrumentation/src/init.rs), [`hermes-instrumentation/README.md`](hermes-instrumentation/README.md)

### 4b. TypeScript Services (Sentry + Effect OTel)

**api** ([`api/src/services/telemetry.ts`](api/src/services/telemetry.ts)):
- `@sentry/node` + `@sentry/opentelemetry` + `@effect/opentelemetry`
- `SentrySpanProcessor` bridges OTel spans to Sentry
- Global `BasicTracerProvider` for non-Effect code (GraphQL, HTTP middleware)
- Effect gets its own scoped provider via `NodeSdk.layer`
- Custom `SentryLogger` replaces Effect's default logger: ERROR/FATAL create Sentry issues, others become breadcrumbs
- Sentry initialized eagerly at module load time

**proposal-executor** ([`proposal-executor/src/telemetry.ts`](proposal-executor/src/telemetry.ts)):
- Same architecture as api (OTel + Sentry + Effect), adapted for short-lived CronJob
- Includes `flush` Effect that drains both OTel spans and Sentry events before `process.exit()`
- Dual-writes to console (primary) and Sentry (supplementary) — CronJob pods rely on `kubectl logs`

### Why Dual Export (Sentry + Axiom)

Sentry applies server-side Dynamic Sampling even when the SDK sends 100% of traces. This makes it unreliable for finding specific traces by identifier. Axiom stores 100% of traces via OTLP batch export, providing reliable trace lookup. Sentry remains the primary tool for error correlation and issue tracking.

### Tracing Configuration by Service (Verified Against K8s YAMLs)

| Service | Sentry DSN | Axiom Export | Notes |
|---------|-----------|-------------|-------|
| api | ✓ (`api-sentry` secret) | ✗ | TypeScript, Effect OTel |
| atlas | ✓ (`atlas-otel` secret) | ✗ | Rust, hermes-instrumentation |
| hermes-pipeline | ✓ (`hermes-pipeline-secrets`) | ✓ (`hermes-pipeline-secrets`) | Rust, hermes-instrumentation |
| hermes-ipfs-cache | ✓ (`hermes-ipfs-cache-secrets`) | ✓ (`hermes-ipfs-cache-secrets`) | Rust, hermes-instrumentation |
| kg-indexer | ✓ (`kg-indexer-secrets`) | ✓ (`kg-indexer-secrets`) | Rust, hermes-instrumentation |
| search-indexer | ✓ (`search-indexer-secrets`) | ✗ | Rust, hermes-instrumentation |
| vote-indexer | ✓ (`vote-indexer-secrets`) | ✗ | Rust, hermes-instrumentation |
| scoring-service | ✓ (`scoring-cronjob-secrets`) | ✗ | Rust, hermes-instrumentation |
| proposal-executor | ✓ (`proposal-executor-credentials`) | ✗ | TypeScript, Effect OTel |
| kafka-ui | ✗ | ✗ | Third-party image, no telemetry |
| actions-indexer | ✗ | ✗ | No K8s deployment config |

## 5. Health Checks

### API Health Endpoints

The API exposes 6 health endpoints under `/health` ([`api/src/health.ts`](api/src/health.ts)):

**K8s probes:**

| Endpoint | Purpose | Dependencies |
|----------|---------|-------------|
| `/health/liveness` | Event loop responsive | None |
| `/health/readiness` | Ready for traffic | DB reachability, GraphQL pool saturation |

**Debugging endpoints:** `/health/` (basic DB check), `/health/detailed` (full pool diagnostics), `/health/graphql-pool` (PostGraphile pool), `/health/pool` (Drizzle pool)

### DB Saturation Detection

[`api/src/services/dbSaturation.ts`](api/src/services/dbSaturation.ts) — Hysteresis-based activation/release that feeds into the readiness probe:

- **Pressure signals:** Waiting clients ≥ 1, pool utilization ≥ 90%, acquire timeouts ≥ 2 in 30s window
- **Activation:** Sustained pressure for 15s → `isSaturated = true` → readiness probe returns 503
- **Release:** No pressure for 30s → `isSaturated = false` → readiness probe returns 200
- Configurable via env vars: `PG_POOL_PRESSURE_WAITING_THRESHOLD`, `PG_POOL_PRESSURE_UTILIZATION_THRESHOLD`, `PG_POOL_PRESSURE_TIMEOUT_THRESHOLD`, `PG_POOL_SATURATION_ACTIVATION_MS`, `PG_POOL_SATURATION_RELEASE_MS`, `PG_POOL_ACQUIRE_TIMEOUT_WINDOW_MS`

### Search-Indexer Health

- **Liveness:** HTTP GET `/healthz` on port 8080 (process alive)
- **Readiness:** HTTP GET `/readyz` on port 8080 (OpenSearch + Kafka connectivity)

### CronJob Health Model

`proposal-executor` and `scoring-service` use CronJob-based health instead of probes:

| CronJob | Schedule | restartPolicy | activeDeadlineSeconds | backoffLimit |
|---------|----------|---------------|----------------------|-------------|
| proposal-executor | Every 5 min | Never | 290 (10s gap before next run) | 1 |
| scoring-service | 3am daily | OnFailure | — | — |

### Services Without Health Probes

All other services (atlas, hermes-pipeline, kg-indexer, hermes-ipfs-cache, vote-indexer, kafka-ui, actions-indexer) have no health probes configured. These services will not be automatically restarted if they hang or become unresponsive — failures are only detected via alerts (atlas) or not at all.

## 6. Per-Service Reference

| Service | Namespace | Alerts | Dashboard | Sentry | Axiom | Health Probe |
|---------|-----------|--------|-----------|--------|-------|-------------|
| actions-indexer | — | — | — | ✗ | ✗ | — |
| api | `api` | 3 (§3) | API Ingress (§2) | ✓ | ✗ | liveness, readiness |
| atlas | `knowledge` | 5 prod + 5 staging (§3) | Atlas Overview (§2) | ✓ | ✗ | — |
| hermes-ipfs-cache | `knowledge` | — | — | ✓ | ✓ | — |
| hermes-pipeline | `knowledge` | — | — | ✓ | ✓ | — |
| kafka-ui | `kafka` | — | — | ✗ | ✗ | — |
| kg-indexer | `knowledge` | — | — | ✓ | ✓ | — |
| proposal-executor | `knowledge` | — | — | ✓ | ✗ | CronJob (§5) |
| scoring-service | `scoring` | — | — | ✓ | ✗ | CronJob (§5) |
| search-indexer | `search` | — | OpenSearch (§2) | ✓ | ✗ | liveness, readiness |
| vote-indexer | `scoring` | — | — | ✓ | ✗ | — |

Section references (§) point to where each feature is documented in detail.

## 7. Kubernetes Monitoring

### Disabled Components

`kubeEtcd`, `kubeControllerManager`, `kubeScheduler`, `kubeProxy` are disabled — DigitalOcean manages these internally and they're not accessible.

**Config:** [`monitoring/values.yaml`](monitoring/values.yaml)

### HPA (Horizontal Pod Autoscaler)

The API deployment uses a multi-metric HPA ([`api/k8s/production/hpa.yaml`](api/k8s/production/hpa.yaml)):

| Metric | Type | Target |
|--------|------|--------|
| CPU utilization | Resource | 70% average |
| Memory utilization | Resource | 80% average |
| `api_ingress_503_ratio_rate5m` | External | 1.5% (value: `15m` = 0.015) |

- **Min replicas:** 2
- **Max replicas:** 6

### Prometheus Adapter

[`monitoring/k8s/prometheus-adapter-config.yaml`](monitoring/k8s/prometheus-adapter-config.yaml) bridges Prometheus recording rules to the K8s external metrics API:

```
Recording Rule → Prometheus → Prometheus Adapter → external.metrics.k8s.io API → HPA
```

The adapter exposes `api_ingress_503_ratio_rate5m` as an external metric by computing the 503 ratio from raw `nginx_ingress_controller_requests` metrics (independently from the recording rule, because the adapter's `externalRules` require a `seriesQuery`/`metricsQuery` pair). The HPA references this metric to scale based on error rate.

### Ingress-NGINX Metrics

[`monitoring/k8s/ingress-nginx-metrics.yaml`](monitoring/k8s/ingress-nginx-metrics.yaml) creates a Service targeting the ingress-nginx controller's metrics port (10254) and a ServiceMonitor so Prometheus scrapes it at 30s intervals.

## 8. Access Guide

### Cluster-Wide Stack (monitoring namespace)

**Grafana:**

```bash
kubectl port-forward -n monitoring svc/kube-prometheus-stack-grafana 3000:80
# Open http://localhost:3000

# Credentials:
# Username: admin
# Password:
kubectl get secret kube-prometheus-stack-grafana -n monitoring \
  -o jsonpath="{.data.admin-password}" | base64 -d && echo
```

**Prometheus:**

```bash
kubectl port-forward -n monitoring svc/kube-prometheus-stack-prometheus 9090:9090
# Open http://localhost:9090
```

**Alertmanager:**

```bash
kubectl port-forward -n monitoring svc/kube-prometheus-stack-alertmanager 9093:9093
# Open http://localhost:9093
```

### Search-Namespace Stack (search namespace)

**Grafana:** Exposed via NodePort `30440` on port 4040.

```bash
# If node IP is accessible:
# Open http://<node-ip>:30440

# Otherwise, port-forward:
kubectl port-forward -n search svc/grafana 4040:4040
# Open http://localhost:4040

# Credentials: from grafana-credentials secret in search namespace
kubectl get secret grafana-credentials -n search \
  -o jsonpath="{.data.ADMIN_PASSWORD}" | base64 -d && echo
```

**Prometheus:**

```bash
# Note: uses port 9091 to avoid conflict with cluster-wide Prometheus on 9090
kubectl port-forward -n search svc/prometheus 9091:9090
# Open http://localhost:9091
```

### Kafka UI

```bash
kubectl port-forward -n kafka svc/kafka-ui 8080:8080
# Open http://localhost:8080
```

**Config:** [`monitoring/README.md`](monitoring/README.md), [`search-indexer-deploy/k8s/production/monitoring.yaml`](search-indexer-deploy/k8s/production/monitoring.yaml)

## 9. Daily Metrics Report

[`monitoring/daily-metrics.sh`](monitoring/daily-metrics.sh) — collects API namespace metrics and posts a summary to Slack via webhook.

**Scope:** API namespace only (not cluster-wide).

### What It Collects

1. **Pod count and readiness** — running pods in `api` namespace
2. **HPA status** — current/max replicas, warns if at max
3. **CPU usage** — 5m rate per pod, top 5 + 24h peaks
4. **CPU throttling** — percentage per pod, flags >5% (red circle >25%), 24h peaks
5. **Memory usage** — working set per pod with % of limit, warns >80%, 24h peaks
6. **Restarts** — 24h restart count per pod
7. **Node memory** — cluster-wide node memory utilization, warns >70%, red >85%, 24h peaks
8. **OOM kills** — detected OOMKilled pods in api namespace
9. **Insights** — auto-generated analysis (throttling, memory trends, HPA saturation, restarts)

### How to Run

```bash
# Requires: .env file in monitoring/ with SLACK_WEBHOOK_URL
# Requires: kubectl access to the cluster

cd monitoring
./daily-metrics.sh
```

The script port-forwards to Prometheus (`svc/kube-prometheus-stack-prometheus` on port 9091), queries metrics via the HTTP API, and posts a formatted Slack message.
