#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Load .env
if [[ -f "$SCRIPT_DIR/.env" ]]; then
  source "$SCRIPT_DIR/.env"
else
  echo "Error: $SCRIPT_DIR/.env not found" >&2
  exit 1
fi

if [[ -z "${SLACK_WEBHOOK_URL:-}" ]]; then
  echo "Error: SLACK_WEBHOOK_URL not set in .env" >&2
  exit 1
fi

LOCAL_PORT=9091
PROM_SVC="svc/kube-prometheus-stack-prometheus"
PROM_NS="monitoring"

# --- Port-forward setup/teardown ---
cleanup() {
  kill "$PF_PID" 2>/dev/null || true
}
trap cleanup EXIT

kubectl port-forward "$PROM_SVC" "$LOCAL_PORT":9090 -n "$PROM_NS" >/dev/null 2>&1 &
PF_PID=$!
sleep 3

PROM="http://localhost:$LOCAL_PORT"

query() {
  curl -sf "$PROM/api/v1/query" --data-urlencode "query=$1"
}

# --- Collect metrics ---
SUMMARY=$(python3 - "$PROM" <<'PYEOF'
import json, urllib.request, urllib.parse, sys, time
from datetime import datetime, timezone, timedelta

PROM = sys.argv[1]
PT = timezone(timedelta(hours=-8))

def query(expr):
    url = f"{PROM}/api/v1/query?{urllib.parse.urlencode({'query': expr})}"
    with urllib.request.urlopen(url, timeout=10) as resp:
        data = json.load(resp)
    if data["status"] != "success":
        return []
    return data["data"]["result"]

def query_range(expr, range_hours=24, step="300s"):
    end = int(time.time())
    start = end - (range_hours * 3600)
    params = urllib.parse.urlencode({"query": expr, "start": start, "end": end, "step": step})
    url = f"{PROM}/api/v1/query_range?{params}"
    with urllib.request.urlopen(url, timeout=15) as resp:
        data = json.load(resp)
    if data["status"] != "success":
        return []
    return data["data"]["result"]

def ts_pt(unix_ts):
    """Format unix timestamp as Pacific time."""
    return datetime.fromtimestamp(unix_ts, tz=PT).strftime("%-I:%M %p PT")

def fmt_mib(val):
    return f"{float(val) / 1024 / 1024:.0f}"

def fmt_pct(val):
    return f"{float(val):.1f}"

def find_peak(results, value_fn=float):
    """Find per-pod peak value and timestamp from a range query result."""
    peaks = {}
    for r in results:
        pod = r["metric"].get("pod", r["metric"].get("instance", "?"))
        short = pod.split("-")[-1] if "-" in pod else pod
        best_val = None
        best_ts = None
        for ts, val in r["values"]:
            v = value_fn(val)
            if best_val is None or v > best_val:
                best_val = v
                best_ts = ts
        if best_val is not None:
            peaks[short] = (best_val, best_ts)
    return peaks

lines = []
lines.append("*Daily Cluster Metrics Summary*")
lines.append(f"_{datetime.now().strftime('%Y-%m-%d %H:%M %Z')}_\n")

# --- Pod count and readiness ---
pods = query('kube_pod_status_phase{namespace="api",phase="Running"}')
pod_count = len(pods)
lines.append(f"*Pods:* {pod_count} running")

# --- HPA ---
hpa_current = query('kube_horizontalpodautoscaler_status_current_replicas{namespace="api"}')
hpa_max = query('kube_horizontalpodautoscaler_spec_max_replicas{namespace="api"}')
if hpa_current and hpa_max:
    cur = int(float(hpa_current[0]["value"][1]))
    mx = int(float(hpa_max[0]["value"][1]))
    flag = " :warning:" if cur >= mx else ""
    lines.append(f"*HPA:* {cur}/{mx} replicas{flag}")

# --- CPU ---
lines.append("\n*CPU Usage (5m rate)*")
cpu = query('sort_desc(rate(container_cpu_usage_seconds_total{namespace="api",container="api"}[5m]) * 1000)')
for r in cpu[:5]:
    pod = r["metric"].get("pod", "?").split("-")[-1]
    mc = float(r["value"][1])
    lines.append(f"  `{pod}`: {mc:.0f}m")
if len(cpu) > 5:
    lines.append(f"  _...and {len(cpu)-5} more_")

# CPU peaks (24h)
cpu_range = query_range('rate(container_cpu_usage_seconds_total{namespace="api",container="api"}[5m]) * 1000')
cpu_peaks = find_peak(cpu_range)
if cpu_peaks:
    top3 = sorted(cpu_peaks.items(), key=lambda x: x[1][0], reverse=True)[:3]
    lines.append(f"  _24h peaks:_ " + ", ".join(
        f"`{pod}` {val:.0f}m at {ts_pt(ts)}" for pod, (val, ts) in top3
    ))

# --- CPU Throttling ---
lines.append("\n*CPU Throttling*")
throttle = query('sort_desc(rate(container_cpu_cfs_throttled_periods_total{namespace="api",container="api"}[5m]) / rate(container_cpu_cfs_periods_total{namespace="api",container="api"}[5m]) * 100)')
has_throttle = False
for r in throttle:
    pod = r["metric"].get("pod", "?").split("-")[-1]
    pct = float(r["value"][1])
    if pct > 5:
        has_throttle = True
        flag = " :red_circle:" if pct > 25 else ""
        lines.append(f"  `{pod}`: {fmt_pct(r['value'][1])}%{flag}")
if not has_throttle:
    lines.append("  All pods < 5% :white_check_mark:")

# Throttle peaks (24h)
throttle_range = query_range('rate(container_cpu_cfs_throttled_periods_total{namespace="api",container="api"}[5m]) / rate(container_cpu_cfs_periods_total{namespace="api",container="api"}[5m]) * 100')
throttle_peaks = find_peak(throttle_range)
if throttle_peaks:
    worst = sorted(throttle_peaks.items(), key=lambda x: x[1][0], reverse=True)[:3]
    notable = [(pod, val, ts) for pod, (val, ts) in worst if val > 5]
    if notable:
        lines.append(f"  _24h peaks:_ " + ", ".join(
            f"`{pod}` {val:.0f}% at {ts_pt(ts)}" for pod, val, ts in notable
        ))

# --- Memory ---
lines.append("\n*Memory Usage*")
mem = query('sort_desc(container_memory_working_set_bytes{namespace="api",container="api"})')
mem_limit = query('kube_pod_container_resource_limits{namespace="api",container="api",resource="memory"}')
limit_bytes = float(mem_limit[0]["value"][1]) if mem_limit else None
for r in mem[:5]:
    pod = r["metric"].get("pod", "?").split("-")[-1]
    mib = float(r["value"][1]) / 1024 / 1024
    pct_str = ""
    if limit_bytes:
        pct = float(r["value"][1]) / limit_bytes * 100
        flag = " :warning:" if pct > 80 else ""
        pct_str = f" ({pct:.0f}% of limit){flag}"
    lines.append(f"  `{pod}`: {mib:.0f} MiB{pct_str}")
if len(mem) > 5:
    lines.append(f"  _...and {len(mem)-5} more_")

# Memory peaks (24h)
mem_range = query_range('container_memory_working_set_bytes{namespace="api",container="api"}')
mem_peaks = find_peak(mem_range)
if mem_peaks:
    top3 = sorted(mem_peaks.items(), key=lambda x: x[1][0], reverse=True)[:3]
    lines.append(f"  _24h peaks:_ " + ", ".join(
        f"`{pod}` {val/1024/1024:.0f} MiB at {ts_pt(ts)}" for pod, (val, ts) in top3
    ))

# --- Restarts (last 24h) ---
lines.append("\n*Restarts (last 24h)*")
restarts = query('increase(kube_pod_container_status_restarts_total{namespace="api",container="api"}[24h])')
total_restarts = 0
for r in restarts:
    count = float(r["value"][1])
    if count > 0.5:
        pod = r["metric"].get("pod", "?").split("-")[-1]
        total_restarts += int(count)
        lines.append(f"  `{pod}`: {int(count)} restarts")
if total_restarts == 0:
    lines.append("  None :white_check_mark:")

# --- Node memory ---
lines.append("\n*Node Memory*")
nodes = query('(1 - node_memory_MemAvailable_bytes / node_memory_MemTotal_bytes) * 100')
for r in sorted(nodes, key=lambda x: float(x["value"][1]), reverse=True):
    node = r["metric"].get("instance", "?")
    pct = float(r["value"][1])
    flag = " :red_circle:" if pct > 85 else " :warning:" if pct > 70 else ""
    lines.append(f"  `{node}`: {pct:.1f}%{flag}")

# Node memory peaks (24h)
node_range = query_range('(1 - node_memory_MemAvailable_bytes / node_memory_MemTotal_bytes) * 100')
node_peaks = find_peak(node_range)
if node_peaks:
    worst = sorted(node_peaks.items(), key=lambda x: x[1][0], reverse=True)[:3]
    lines.append(f"  _24h peaks:_ " + ", ".join(
        f"`{node}` {val:.1f}% at {ts_pt(ts)}" for node, (val, ts) in worst
    ))

# --- OOM kills ---
oom = query('kube_pod_container_status_last_terminated_reason{namespace="api",reason="OOMKilled"}')
if oom:
    lines.append(f"\n:skull: *OOM Kills detected:* {len(oom)} pods")

# --- Insights ---
lines.append("\n*Insights*")
insights = []

# Check if any throttling is concerning (use peaks for better signal)
peak_throttle = max((v for v, _ in throttle_peaks.values()), default=0) if throttle_peaks else 0
if peak_throttle > 25:
    worst_pod = max(throttle_peaks.items(), key=lambda x: x[1][0])
    insights.append(f":red_circle: CPU throttling peaked at {worst_pod[1][0]:.0f}% (`{worst_pod[0]}` at {ts_pt(worst_pod[1][1])}) — event loop may have stalled")
elif peak_throttle > 10:
    insights.append(f":eyes: CPU throttling peaked at {peak_throttle:.0f}% — worth monitoring")

# Check memory trend (use peaks for better signal)
peak_mem = max((v for v, _ in mem_peaks.values()), default=0) if mem_peaks else 0
if limit_bytes and peak_mem / limit_bytes > 0.8:
    worst_pod = max(mem_peaks.items(), key=lambda x: x[1][0])
    insights.append(f":warning: Memory peaked at {worst_pod[1][0]/1024/1024:.0f} MiB (`{worst_pod[0]}` at {ts_pt(worst_pod[1][1])}) — approaching 1Gi limit")

# Check HPA saturation
if hpa_current and hpa_max:
    if int(float(hpa_current[0]["value"][1])) >= int(float(hpa_max[0]["value"][1])):
        insights.append(":warning: HPA at max replicas — cannot absorb additional load")

# Check restarts
if total_restarts > 3:
    insights.append(f":red_circle: {total_restarts} restarts in 24h — investigate pod logs")
elif total_restarts > 0:
    insights.append(f":eyes: {total_restarts} restart(s) in 24h")

if not insights:
    insights.append(":white_check_mark: Cluster looks healthy. No concerns.")

for insight in insights:
    lines.append(f"  {insight}")

print("\n".join(lines))
PYEOF
)

# --- Post to Slack ---
PAYLOAD=$(python3 -c "
import json, sys
text = sys.stdin.read()
print(json.dumps({'text': text}))
" <<< "$SUMMARY")

HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$SLACK_WEBHOOK_URL" \
  -H "Content-Type: application/json" \
  -d "$PAYLOAD")

if [[ "$HTTP_CODE" == "200" ]]; then
  echo "Summary posted to Slack"
else
  echo "Error posting to Slack (HTTP $HTTP_CODE)" >&2
  exit 1
fi
