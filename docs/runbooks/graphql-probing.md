# GraphQL probing — triage and response

What to do when an `api.security` alert fires, or when you want to know who is mapping the public GraphQL api.

The signals come from `api/src/kg/securitySignalsPlugin.ts`. Alerts are in [`monitoring/k8s/v2/api-security-alerts.yaml`](../../monitoring/k8s/v2/api-security-alerts.yaml), and the **Gaia API Security Signals** dashboard is in [`monitoring/k8s/v2/api-security-dashboard.yaml`](../../monitoring/k8s/v2/api-security-dashboard.yaml).

## What is measured

| signal | meaning | normal? |
|---|---|---|
| `parse_failed` | the body is not GraphQL | rare; our clients are generated, so they never send these |
| `validation_failed` | valid GraphQL the schema rejects | some, from stale or buggy clients. `unknown_field` is the scanner-shaped subset |
| `hidden_surface_probe` | names something in `HIDDEN_SURFACE` (`securitySignals.ts`), i.e. schema we removed on purpose | **never** from a first-party client |
| `introspection` | a successful `__schema` / `__type` query | yes: GraphiQL and codegen. Context, not an alert |

Each signal is handled three ways, each with its own budget:

- **Prometheus counters** count every event. Labels are fixed sets and never include an IP.
- **A structured `warn` log line** (`"GraphQL security signal"`) records the caller and what they asked for. Each address gets its first 20 lines in full, then one every 5 seconds. Each pod logs at most 20 lines/s across all addresses. Whatever is withheld is counted in the next line's `suppressedSinceLastEmit` and in `gaia_api_security_signal_log_suppressed_total`.
- **A Sentry issue** (`"Hidden GraphQL surface probed"`) is raised for hidden-surface probes only: at most once per address per hour, and 5 per ten minutes overall.

The caller is the rightmost `X-Forwarded-For` entry, the one the Cilium Gateway appends (`api/src/utils/clientIp.ts`). `X-Real-IP` is ignored, because the Gateway passes it through unchanged, so a caller can't choose their own address.

## First five minutes

1. **Who and what.** Every pod's signal lines, newest last:
   ```bash
   kubectl -n gaia logs -l app=api --since=30m --prefix --max-log-requests=20 \
     | grep '"GraphQL security signal"' | sed 's/^[^{]*//' \
     | jq -c '{signal, clientIp, userAgent, origin, unknownFields, hiddenSurface, episodeEvents, suppressedSinceLastEmit}'
   ```
   Top sources in the window:
   ```bash
   … | jq -r .clientIp | sort | uniq -c | sort -rn | head
   ```
   Pod logs do not survive a pod restart or a deploy. Read them before you roll anything.
2. **Sentry:** issue *Hidden GraphQL surface probed* in `gaia-api`. Its events carry the same fields, and they outlive pods.
3. **Shape:** on the dashboard, *Rejected requests by reason* and *Hidden-surface probes by target* show whether this is one burst, a steady crawl, or a ramp.

## Reading it

- **One address, a cloud provider's range, `Python-urllib` / `Go-http-client` / no origin, many distinct `unknownFields`:** a scanner. If it found nothing (every request rejected), the rejections are the defence working. Record it and move on.
- **Many addresses, browser user agents, our own `origin`, the same few `unknownFields`:** one of our clients is broken by a schema change, typically a field renamed or removed while an old frontend build is still deployed. This is an outage for those users. Find the field, and either restore it or ship the client fix.
- **`hidden_surface_probe`:** someone knows names we removed, from an old schema dump, a cached introspection result, or a leaked client. Check whether the same address also succeeded at anything sensitive (step 1, plus any `"GraphQL search field invoked"` lines for it). If the hidden surface held secrets, assume they were read before it was hidden, and rotate them.
- **`gaia_api_security_signal_source_evictions_total` rising:** more than 10,000 distinct addresses on a pod, which is an address-rotating flood. Memory stays bounded. The per-address log budget no longer limits volume (each new address starts fresh), but the global 20/s ceiling still does.

## Responding

- **Nothing is exposed and the volume is harmless:** no action. Do not chase scanners; they rotate.
- **Volume is hurting the api:** the per-IP rate limiter (`api/src/middleware/rateLimit.ts`, 6,000 req/min by default) is the backstop, and it now keys on an address the caller can't forge. The api has no per-address blocklist today. Blocking at the Cilium Gateway would be the place for one; add it deliberately rather than mid-incident.
- **Something sensitive is reachable:** omit it from the schema (`hidePrivateTablesPlugin.ts` / `hideProceduresPlugin.ts`), add its names to `HIDDEN_SURFACE`, deploy, then rotate anything it exposed. Adding it to `HIDDEN_SURFACE` both asserts it stays out of the schema (`hiddenSchemaSurface.test.ts`) and makes any future request for it trip this alarm.

## When the alerts themselves look wrong

- **`ApiSecuritySignalsMissing`:** the counters are gone. Check `up{job="api"}` and the `gaia-api-metrics` ServiceMonitor, then whether the running image still registers `useSecuritySignals` in `postgraphile.ts`.
- **`ApiSecuritySignalsDegraded`:** detection threw. Requests are unaffected, because every hook is guarded, but counts are low until it's fixed. `kubectl -n gaia logs -l app=api --since=1h | grep 'Security signal detection failed'` has the stage and error. The first version of the analyzer failed this way, on a schema object from a second copy of graphql-js; `SchemaLike` in `securitySignals.ts` explains why it is duck-typed now.
- **`ApiGraphqlRejectionSurge` fires on ordinary traffic:** recalibrate the threshold in `api-security-alerts.yaml` against the dashboard's baseline. It was set at 1/s before any production data existed.
