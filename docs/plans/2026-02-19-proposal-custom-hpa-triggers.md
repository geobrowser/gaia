# proposal: custom HPA triggers for API DB pressure

## Current State

- HPA now includes an external-metric trigger name (`api_ingress_503_ratio_rate5m`) in addition to CPU/memory (`api/k8s/production/hpa.yaml`).
- Cluster does not currently expose custom/external metrics APIs:
  - no `custom.metrics.k8s.io`
  - no `external.metrics.k8s.io`

## Goal

Make autoscaling react to DB-pressure incidents (latency/error/queue signals), not only node resource usage.

## Proposed Trigger Set (Phase 1)

Use one conservative external trigger first:

1. `api:ingress_503_ratio:rate5m`
   - scale out when sustained above threshold (example 1.5%)
2. Keep CPU metric as a safety net

Rationale: this is already recorded in Prometheus rules and is easy to reason about operationally.

## Implementation Plan

### 1) Deploy Prometheus Adapter

Install `prometheus-adapter` in `monitoring` namespace and configure external metric rules mapping PromQL to metric names.

Expected post-install check:

```bash
kubectl get apiservices | rg "external.metrics|custom.metrics"
```

### 2) Add adapter mapping rule for API 503 ratio

Map this PromQL to an external metric name, example:

- metric name: `api_ingress_503_ratio_rate5m`
- query source: `api:ingress_503_ratio:rate5m`

### 3) Update production HPA with external metric

Add an `External` metric entry while keeping existing CPU/memory metrics:

```yaml
- type: External
  external:
    metric:
      name: api_ingress_503_ratio_rate5m
    target:
      type: Value
      value: "0.015"
```

### 4) Validate end-to-end

```bash
kubectl get --raw "/apis/external.metrics.k8s.io/v1beta1" | jq
kubectl describe hpa api -n api
```

Verify HPA status shows external metric current/target values and scale recommendations.

### 5) Rollout strategy

1. deploy adapter
2. deploy HPA external metric trigger with conservative threshold
3. monitor for 24-48h
4. tighten threshold if needed

## Guardrails

- Keep `maxReplicas` bounded by connection budget.
- Use longer downscale stabilization than upscale.
- Avoid multiple aggressive external triggers in first rollout.

## Phase 2 (optional)

After proving the first trigger, add one internal DB-pressure signal (pool waiting/acquire timeouts) as a second external metric.
