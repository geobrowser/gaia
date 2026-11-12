#!/usr/bin/env bash
# Offset arithmetic and guard tests for geodb/kafka-rewind.sh. Offline: no cluster, no broker.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
fx="$here/testdata/kafka-rewind"
# shellcheck disable=SC1091
source "$here/kafka-rewind.sh"
fail=0
check() { if [[ "$2" == "$3" ]]; then echo "ok   $1"; else echo "FAIL $1"; echo "  want: $3"; echo "  got:  $2"; fail=1; fi; }

# 1. the plan: at-timestamp, no message after ts -> log end, ts before retention -> earliest
check "plan arithmetic" "$(compute_plan "$fx/timestamp.txt" "$fx/earliest.txt" "$fx/latest.txt")" "$(cat "$fx/expected-plan.txt")"

# 2. the --from-file CSV has exactly topic,partition,offset
check "reset csv" "$(compute_plan "$fx/timestamp.txt" "$fx/earliest.txt" "$fx/latest.txt" | plan_csv)" "$(cut -d, -f1-3 "$fx/expected-plan.txt")"

# 3. a partition missing its earliest/latest bounds is an error, not a guess
printf 'testnet.knowledge.edits:0:1\ntestnet.knowledge.edits:9:5\n' >"$fx/../.bad-ts"
if compute_plan "$fx/../.bad-ts" "$fx/earliest.txt" "$fx/latest.txt" >/dev/null 2>&1; then check "missing bounds rejected" accepted rejected; else check "missing bounds rejected" rejected rejected; fi
rm -f "$fx/../.bad-ts"

# 4. timestamps
check "iso to ms" "$(to_epoch_ms 2026-11-12T06:30:00Z)" "1794465000000"
check "epoch ms passthrough" "$(to_epoch_ms 1794465000000)" "1794465000000"
if (to_epoch_ms "yesterday") >/dev/null 2>&1; then check "bad timestamp rejected" accepted rejected; else check "bad timestamp rejected" rejected rejected; fi

# 5. the three groups map to the manifest group ids, and an unknown one is refused
check "kg group" "$(group_info kg-indexer-v8-testnet | cut -d'|' -f1)" kg-indexer
check "vote group" "$(group_info vote-indexer-testnet | cut -d'|' -f1)" vote-indexer
check "topology group" "$(group_info topology-indexer-testnet | cut -d'|' -f1)" topology-indexer
if group_info kg-indexer >/dev/null 2>&1; then check "unknown group rejected" accepted rejected; else check "unknown group rejected" rejected rejected; fi

# 6. the CLI: offline dry run prints the plan and the command and never executes; --yes offline is refused
out=$("$here/kafka-rewind.sh" --group kg-indexer-v8-testnet --to 2026-11-12T06:30:00Z --plan-from "$fx")
if grep -q 'DRY RUN: nothing was changed' <<<"$out" && grep -q -- '--reset-offsets --from-file /tmp/plan.csv --execute' <<<"$out"; then
  check "dry run output" ok ok; else check "dry run output" bad ok; fi
if "$here/kafka-rewind.sh" --group kg-indexer-v8-testnet --to 2026-11-12T06:30:00Z --plan-from "$fx" --yes >/dev/null 2>&1; then
  check "--yes refused offline" accepted refused; else check "--yes refused offline" refused refused; fi

# 7. --yes with the consumer still running is refused before any pod is created (stub kubectl: 1 replica, 1 pod)
stub=$(mktemp -d)
cat >"$stub/kubectl" <<'STUB'
#!/bin/sh
case "$*" in
  *"get deployment"*) printf 1 ;;
  *"get pods"*) echo pod/kg-indexer-abc ;;
  *) echo "unexpected kubectl call: $*" >&2; exit 9 ;;
esac
STUB
chmod +x "$stub/kubectl"
rc=0; msg=$(PATH="$stub:$PATH" "$here/kafka-rewind.sh" --group kg-indexer-v8-testnet --to 2026-11-12T06:30:00Z --yes 2>&1) || rc=$?
if [[ $rc -ne 0 ]] && grep -q 'REFUSING: kg-indexer is not scaled to zero' <<<"$msg"; then check "--yes refused while running" refused refused; else check "--yes refused while running" "rc=$rc $msg" refused; fi
rm -rf "$stub"

exit "$fail"
