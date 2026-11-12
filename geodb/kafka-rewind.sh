#!/usr/bin/env bash
# Rewind a gaia Kafka consumer group to a point in time (RUNBOOK section 13).
#
# Why: with a writer off-switch such as KG_INDEXER_PG_WRITES=0 the consumer still commits its offsets, so
# flipping the flag back does not restore the window Postgres missed. Rewinding the group to the flip
# time replays it (the writes are idempotent per block).
#
#   geodb/kafka-rewind.sh --group kg-indexer-v8-testnet --to 2026-11-12T06:30:00Z          # dry run
#   geodb/kafka-rewind.sh --group kg-indexer-v8-testnet --to 2026-11-12T06:30:00Z --yes    # executes
#
# Dry run (the default) reads the broker's offsets for the timestamp through a throwaway pod (it only
# reads), prints the per-partition target offsets and the exact reset command, and changes nothing.
# With --yes the script refuses unless the consumer Deployment is at 0 replicas with no pods left, then
# resets the group from the printed plan. It never scales anything: scaling is the operator's step.
#
# Groups come from the v2 manifests (KAFKA_GROUP_ID): kg-indexer, vote-indexer, topology-indexer.
# Topics are the ones each binary subscribes to, with the ENVIRONMENT prefix ("testnet.").
#
# Offline use (no cluster, used by geodb/test-kafka-rewind.sh): --plan-from DIR, where DIR holds
# timestamp.txt, earliest.txt and latest.txt in kafka-get-offsets.sh format (topic:partition:offset).
set -euo pipefail

CONTEXT="${KAFKA_REWIND_CONTEXT:-do-nyc2-geo-testnet-k8s}"
NAMESPACE="${KAFKA_REWIND_NAMESPACE:-gaia}"
IMAGE="${KAFKA_REWIND_IMAGE:-apache/kafka:latest}"   # the image the replay recipe and docker-compose use

die() { echo "kafka-rewind: $*" >&2; exit 2; }

usage() {
  cat >&2 <<USAGE
usage: $0 --group GROUP --to TIMESTAMP [--yes] [--context C] [--namespace N] [--image I] [--plan-from DIR]
  GROUP      kg-indexer-v8-testnet | vote-indexer-testnet | topology-indexer-testnet
  TIMESTAMP  UTC, ISO 8601 (2026-11-12T06:30:00Z) or epoch milliseconds
USAGE
  exit 2
}

# group -> "deployment|secret|topics (comma separated)"
group_info() {
  case "$1" in
    kg-indexer-v8-testnet)
      echo "kg-indexer|kg-indexer-secrets|testnet.hermes.blocks,testnet.knowledge.edits,testnet.space.creations,testnet.space.membership,testnet.space.trust.extensions,testnet.space.topics,testnet.space.governance" ;;
    vote-indexer-testnet)
      echo "vote-indexer|kafka-credentials|testnet.curation.votes" ;;
    topology-indexer-testnet)
      echo "topology-indexer|kafka-credentials|testnet.topology.canonical" ;;
    *) return 1 ;;
  esac
}

# ISO 8601 UTC or epoch milliseconds -> epoch milliseconds
to_epoch_ms() {
  local t="$1"
  if [[ "$t" =~ ^[0-9]{12,13}$ ]]; then echo "$t"; return; fi
  [[ "$t" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$ ]] || die "bad timestamp '$t' (want 2026-11-12T06:30:00Z or epoch ms)"
  local s
  if s=$(date -u -d "$t" +%s 2>/dev/null); then :; else s=$(date -u -j -f '%Y-%m-%dT%H:%M:%SZ' "$t" +%s) || die "cannot parse '$t'"; fi
  echo "${s}000"
}

# The offset arithmetic. Inputs are three files of "topic:partition:offset" lines from kafka-get-offsets.sh:
#   $1 timestamp  (--time <ms>): first offset whose message timestamp >= ms; EMPTY when no such message
#   $2 earliest   (--time -2):   first retained offset
#   $3 latest     (--time -1):   log end offset (next offset to be written)
# Output, one line per partition, sorted: "topic,partition,target,why"
#   why = at-timestamp | clamped-to-earliest (target fell before retention) | at-end (no message at or after ts)
compute_plan() {
  awk -F: '
    FILENAME == ARGV[1] { ts[$1 ":" $2] = $3; keys[$1 ":" $2] = 1; next }
    FILENAME == ARGV[2] { early[$1 ":" $2] = $3; next }
    FILENAME == ARGV[3] { late[$1 ":" $2] = $3; next }
    END {
      for (k in keys) {
        if (!(k in early) || !(k in late)) { print "missing earliest/latest for " k > "/dev/stderr"; bad = 1; continue }
        split(k, p, ":")
        if (ts[k] == "")              { t = late[k];  why = "at-end" }
        else if (ts[k] + 0 < early[k] + 0) { t = early[k]; why = "clamped-to-earliest" }
        else                          { t = ts[k];    why = "at-timestamp" }
        if (t + 0 > late[k] + 0)      { t = late[k]; why = "at-end" }
        printf "%s,%s,%s,%s\n", p[1], p[2], t, why
      }
      if (bad) exit 1
    }' "$1" "$2" "$3" | sort -t, -k1,1 -k2,2n
}

# --from-file input of kafka-consumer-groups.sh: topic,partition,offset
plan_csv() { cut -d, -f1-3; }

print_plan() {
  local plan="$1"
  echo "per-partition target offsets:"
  printf '  %-40s %9s %12s  %s\n' TOPIC PARTITION TARGET WHY
  while IFS=, read -r topic part off why; do
    printf '  %-40s %9s %12s  %s\n' "$topic" "$part" "$off" "$why"
  done <<<"$plan"
  if grep -q ',at-end$' <<<"$plan"; then
    echo "note: at-end means no message at or after the timestamp, so that partition has nothing to replay."
  fi
  if grep -q ',clamped-to-earliest$' <<<"$plan"; then
    echo "note: clamped-to-earliest means the timestamp is older than retention; part of the window is gone."
  fi
}

reset_command() {
  local group="$1"
  echo "kafka-consumer-groups.sh --bootstrap-server \$KAFKA_BROKER --command-config /tmp/client.properties \\"
  echo "  --group $group --reset-offsets --from-file /tmp/plan.csv --execute"
}

# --- cluster side -----------------------------------------------------------------------------------

K() { kubectl --context "$CONTEXT" -n "$NAMESPACE" "$@"; }

# kubectl against these clusters flakes with Unauthorized or a timeout; retry before giving up.
retry() {
  local n
  for n in 1 2 3; do
    if "$@"; then return 0; fi
    echo "kafka-rewind: attempt $n failed: $*" >&2
    sleep 3
  done
  return 1
}

# A throwaway pod that reads the broker and credentials from the consumer's own Secret by reference
# (the script never reads a Secret value). $1 = shell script to run in it, $2 = value of PLAN_CSV.
run_in_pod() {
  local script="$1" plan="${2:-}" pod out mf
  mf=$(mktemp)
  pod="kafka-rewind-$(date -u +%m%d%H%M%S)-$RANDOM"
  {
    cat <<YAML
apiVersion: v1
kind: Pod
metadata:
  name: $pod
  labels: {app: kafka-rewind}
spec:
  restartPolicy: Never
  containers:
    - name: kafka
      image: $IMAGE
      command: ["/bin/sh", "-c"]
      args:
        - |
YAML
    {
      cat <<'PRELUDE'
set -eu
printf '%s' "$KAFKA_SSL_CA_PEM" > /tmp/ca.pem
cat > /tmp/client.properties <<EOP
security.protocol=SASL_SSL
sasl.mechanism=PLAIN
ssl.truststore.type=PEM
ssl.truststore.location=/tmp/ca.pem
sasl.jaas.config=org.apache.kafka.common.security.plain.PlainLoginModule required username="$KAFKA_USERNAME" password="$KAFKA_PASSWORD";
EOP
printf '%s\n' "$PLAN_CSV" > /tmp/plan.csv
PRELUDE
      printf '%s\n' "$script"
    } | sed 's/^/          /'
    echo "      env:"
    local v
    for v in KAFKA_BROKER KAFKA_USERNAME KAFKA_PASSWORD KAFKA_SSL_CA_PEM; do
      echo "        - name: $v"
      echo "          valueFrom: {secretKeyRef: {name: $SECRET, key: $v}}"
    done
    echo "        - name: PLAN_CSV"
    echo "          value: |"
    printf '%s\n' "$plan" | sed 's/^/            /'
  } >"$mf"
  retry K create -f "$mf" >/dev/null || { rm -f "$mf"; die "could not create the throwaway pod"; }
  rm -f "$mf"
  # From here the pod is ours: always delete it, and only it.
  trap 'K delete pod "'"$pod"'" --ignore-not-found --wait=false >/dev/null 2>&1 || true' RETURN
  local phase
  for _ in $(seq 1 90); do
    phase=$(K get pod "$pod" -o jsonpath='{.status.phase}' 2>/dev/null || true)
    [[ "$phase" == Succeeded || "$phase" == Failed ]] && break
    sleep 2
  done
  out=$(retry K logs "$pod") || out=""
  if [[ "$phase" != Succeeded ]]; then
    echo "$out" >&2
    die "pod $pod ended in phase '${phase:-unknown}'"
  fi
  printf '%s\n' "$out"
}

fetch_offsets() {   # $1 = ms; writes timestamp.txt, earliest.txt, latest.txt into $2
  local ms="$1" dir="$2" out
  out=$(run_in_pod "
for t in $TOPICS_SPACED; do
  for w in $ms -2 -1; do
    /opt/kafka/bin/kafka-get-offsets.sh --bootstrap-server \"\$KAFKA_BROKER\" --command-config /tmp/client.properties --topic \$t --time \$w | sed \"s/^/\$w /\"
  done
done")
  : >"$dir/timestamp.txt"; : >"$dir/earliest.txt"; : >"$dir/latest.txt"
  while read -r w line; do
    [[ -n "${line:-}" ]] || continue
    case "$w" in
      -2) echo "$line" >>"$dir/earliest.txt" ;;
      -1) echo "$line" >>"$dir/latest.txt" ;;
      *)  echo "$line" >>"$dir/timestamp.txt" ;;
    esac
  done <<<"$out"
  [[ -s "$dir/latest.txt" ]] || die "broker returned no offsets"
}

consumer_scaled_to_zero() {
  local spec pods
  spec=$(retry K get deployment "$DEPLOYMENT" -o jsonpath='{.spec.replicas}') || return 2
  pods=$(retry K get pods -l "app=$DEPLOYMENT" -o name) || return 2
  echo "deployment/$DEPLOYMENT spec.replicas=${spec:-0}, pods: ${pods:-none}"
  [[ "${spec:-0}" == 0 && -z "$pods" ]]
}

# --- main -------------------------------------------------------------------------------------------

main() {
  local GROUP="" TO="" YES=0 PLAN_FROM=""
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --group) GROUP="${2:-}"; shift 2 ;;
      --to) TO="${2:-}"; shift 2 ;;
      --yes) YES=1; shift ;;
      --context) CONTEXT="${2:-}"; shift 2 ;;
      --namespace) NAMESPACE="${2:-}"; shift 2 ;;
      --image) IMAGE="${2:-}"; shift 2 ;;
      --plan-from) PLAN_FROM="${2:-}"; shift 2 ;;
      -h|--help) usage ;;
      *) usage ;;
    esac
  done
  [[ -n "$GROUP" && -n "$TO" ]] || usage
  local info
  info=$(group_info "$GROUP") || die "unknown group '$GROUP' (known: kg-indexer-v8-testnet, vote-indexer-testnet, topology-indexer-testnet)"
  IFS='|' read -r DEPLOYMENT SECRET TOPICS <<<"$info"
  TOPICS_SPACED="${TOPICS//,/ }"
  local ms; ms=$(to_epoch_ms "$TO")

  # Refuse before touching the broker: --yes needs the consumer at zero.
  local state
  if [[ "$YES" == 1 ]]; then
    [[ -z "$PLAN_FROM" ]] || die "--yes cannot be combined with --plan-from (offline plans never execute)"
    if ! state=$(consumer_scaled_to_zero); then
      echo "$state" >&2
      die "REFUSING: $DEPLOYMENT is not scaled to zero. Run: kubectl --context $CONTEXT -n $NAMESPACE scale deployment/$DEPLOYMENT --replicas=0 and wait for its pods to go."
    fi
    echo "scaled to zero: $state"
  fi

  local work; work=$(mktemp -d); trap 'rm -rf "$work"' EXIT
  if [[ -n "$PLAN_FROM" ]]; then
    cp "$PLAN_FROM/timestamp.txt" "$PLAN_FROM/earliest.txt" "$PLAN_FROM/latest.txt" "$work/"
  else
    fetch_offsets "$ms" "$work"
  fi
  local plan; plan=$(compute_plan "$work/timestamp.txt" "$work/earliest.txt" "$work/latest.txt")
  [[ -n "$plan" ]] || die "empty plan"

  echo "group:       $GROUP"
  echo "consumer:    deployment/$DEPLOYMENT ($NAMESPACE, context $CONTEXT)"
  echo "rewind to:   $TO (epoch ms $ms)"
  echo
  print_plan "$plan"
  echo
  echo "reset command (runs inside a throwaway $IMAGE pod; /tmp/plan.csv is the plan above as topic,partition,offset):"
  reset_command "$GROUP"
  echo

  if [[ "$YES" != 1 ]]; then
    echo "DRY RUN: nothing was changed. Scale $DEPLOYMENT to 0, then re-run with --yes to execute."
    exit 0
  fi
  run_in_pod "
/opt/kafka/bin/kafka-consumer-groups.sh --bootstrap-server \"\$KAFKA_BROKER\" --command-config /tmp/client.properties --group $GROUP --reset-offsets --from-file /tmp/plan.csv --execute
/opt/kafka/bin/kafka-consumer-groups.sh --bootstrap-server \"\$KAFKA_BROKER\" --command-config /tmp/client.properties --group $GROUP --describe" "$(plan_csv <<<"$plan")"
  echo
  echo "Reset done. Next: scale $DEPLOYMENT back up, then check lag returns to 0 (RUNBOOK section 13)."
}

# Sourcing the file (the test does) loads the functions without running main.
if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then main "$@"; fi
