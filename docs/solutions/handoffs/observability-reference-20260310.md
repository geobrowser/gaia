# Handoff: Observability Reference Document

**Epic:** ep-4c459d · **Date:** 2026-03-10 · **Branch:** `docs/observability-reference`

## What Was Done

Created `docs/observability.md` — a 361-line reference document mapping the entire monitoring and observability landscape across Gaia's 11 services and K8s infrastructure.

9 sections: overview/namespace topology, metrics/dashboards, alerting, tracing, health checks, per-service summary table, K8s monitoring, access guide, daily metrics report.

## Key Decisions

- **Single summary table for per-service reference (§6)** instead of 11 individual subsections. Initial draft had verbose per-service cards (~135 lines) that duplicated information from topical sections. Three reviewers independently flagged the redundancy. Collapsed to a single table with section cross-references.
- **Compact recording rules summary** instead of a full 16-row table transcribing the YAML. The YAML file is linked for anyone who needs exact rule names.
- **Verified all claims against actual K8s deployment YAMLs** — not relying on the brainstorm doc's estimates. Key corrections vs brainstorm: atlas does NOT have Axiom export (no AXIOM_TOKEN in its YAML), actions-indexer has no K8s deployment at all.

## Files Modified

- `docs/observability.md` — the reference document (new file)
- `docs/solutions/handoffs/observability-reference-20260310.md` — this handoff

## Verification

- All 34 config file paths referenced in the doc verified to exist
- Tracing configuration (SENTRY_DSN, AXIOM_TOKEN) verified against every service's K8s production YAML
- Health probe claims verified against every service's K8s deployment YAML
- Alert names, severities, and conditions verified against PrometheusRule manifests
- Three-reviewer code review completed and all findings addressed
