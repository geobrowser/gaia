#!/usr/bin/env bash
# Fixture tests for scripts/check-geodb-freeze.py and its wiring in .github/workflows/deploy-v2.yml.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

NOW="2026-11-20T12:00:00Z"
INSIDE="2026-11-20T00:30:00Z"   # 11.5h ago -> inside 24h freeze
OUTSIDE="2026-11-19T11:59:59Z"  # 24h 1s ago -> outside 24h freeze

write_cm() {
	printf '%s\n' "$1" > "$tmp/cm.json"
}

run_check() {
	python3 "$here/check-geodb-freeze.py" --now "$NOW" --configmap-json "$tmp/cm.json" "$@"
}

expect_pass() {
	local desc="$1"; shift
	if ! run_check "$@" >/dev/null 2>&1; then
		echo "FAIL: expected pass ($desc)"
		run_check "$@" || true
		exit 1
	fi
}

expect_fail() {
	local desc="$1"; shift
	if run_check "$@" >/dev/null 2>&1; then
		echo "FAIL: expected refusal ($desc)"
		exit 1
	fi
}

# 1. Missing ConfigMap file (cluster hasn't created geodb-read-source yet) -> pass
python3 "$here/check-geodb-freeze.py" --now "$NOW" --configmap-json "$tmp/missing.json" --service geodb-serve >/dev/null

# 2. Empty ConfigMap with no flip/revert records -> pass
write_cm '{"metadata":{"name":"geodb-read-source"},"data":{"read-source.json":"{\"fields\":{\"entity\":\"shadow\"}}"}}'
expect_pass "empty cm, geodb-serve" --service geodb-serve
expect_pass "empty cm, geodb-feeder" --service geodb-feeder
expect_pass "empty cm, api" --service api

# 3. Annotation inside 24h -> refuses geodb-serve, geodb-feeder, and api; allows unrelated services
write_cm "{\"metadata\":{\"name\":\"geodb-read-source\",\"annotations\":{\"geodb.gaia/last-flip-at\":\"$INSIDE\"}},\"data\":{}}"
expect_fail "annotation inside 24h, geodb-serve" --service geodb-serve
expect_fail "annotation inside 24h, geodb-feeder" --service geodb-feeder
expect_fail "annotation inside 24h, api" --service api
expect_pass "annotation inside 24h, unrelated service valkey" --service valkey
expect_pass "annotation inside 24h, unrelated service atlas" --service atlas

# 4. Annotation outside 24h -> pass
write_cm "{\"metadata\":{\"name\":\"geodb-read-source\",\"annotations\":{\"geodb.gaia/last-flip-at\":\"$OUTSIDE\"}},\"data\":{}}"
expect_pass "annotation outside 24h, geodb-serve" --service geodb-serve
expect_pass "annotation outside 24h, geodb-feeder" --service geodb-feeder
expect_pass "annotation outside 24h, api" --service api

# 5. Auto-revert latch timestamp inside read-source.json inside 24h -> refuses
write_cm "{\"metadata\":{\"name\":\"geodb-read-source\"},\"data\":{\"read-source.json\":\"{\\\"fields\\\":{\\\"entity\\\":\\\"on\\\"},\\\"reverted\\\":{\\\"entity\\\":\\\"$INSIDE error_rate\\\"}}\"}}"
expect_fail "reverted latch inside 24h, geodb-serve" --service geodb-serve
expect_fail "reverted latch inside 24h, geodb-feeder" --service geodb-feeder
expect_fail "reverted latch inside 24h, api" --service api

# 6. lastFlipAt inside read-source.json inside 24h -> refuses
write_cm "{\"metadata\":{\"name\":\"geodb-read-source\"},\"data\":{\"read-source.json\":\"{\\\"lastFlipAt\\\":\\\"$INSIDE\\\",\\\"fields\\\":{\\\"entity\\\":\\\"on\\\"}}\"}}"
expect_fail "lastFlipAt inside 24h, geodb-serve" --service geodb-serve

# 7. Malformed timestamp fails closed (refuses unless overridden)
write_cm '{"metadata":{"name":"geodb-read-source","annotations":{"geodb.gaia/last-flip-at":"not-a-timestamp"}},"data":{}}'
expect_fail "malformed timestamp fails closed" --service geodb-serve

# 8. Named override allows inside window; empty or trivial overrides still refuse
write_cm "{\"metadata\":{\"name\":\"geodb-read-source\",\"annotations\":{\"geodb.gaia/last-flip-at\":\"$INSIDE\"}},\"data\":{}}"
expect_fail "empty override refuses" --service geodb-serve --override ""
expect_fail "whitespace override refuses" --service geodb-serve --override "   "
expect_fail "trivial override true refuses" --service geodb-serve --override "true"
expect_fail "trivial override yes refuses" --service geodb-serve --override "yes"
expect_fail "trivial override 1 refuses" --service geodb-serve --override "1"
expect_pass "named override passes" --service geodb-serve --override "INC-42 emergency OOM fix on serve"

# 9. deploy-v2.yml wires the preflight and exposes freeze_override input
grep -q "freeze_override:" "$repo/.github/workflows/deploy-v2.yml" || { echo "FAIL: deploy-v2.yml missing freeze_override input"; exit 1; }
grep -q "scripts/check-geodb-freeze.py" "$repo/.github/workflows/deploy-v2.yml" || { echo "FAIL: deploy-v2.yml missing check-geodb-freeze.py step"; exit 1; }

echo "check-geodb-freeze tests passed"
