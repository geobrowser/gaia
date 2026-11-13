#!/usr/bin/env bash
# Fixture test for check-env-flags.py: a throwaway repo must pass with a listed or manifest-set flag
# and fail when code reads an unlisted one (and on a stale or forbidden allowlist line).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
cd "$tmp"
git init -q .
mkdir -p svc/src svc/k8s/v2 ci
printf 'fn f(){ let _ = std::env::var("FOO_PG_WRITES"); let _ = std::env::var("BAR_SERVE"); }\n' > svc/src/a.rs
printf 'env:\n  - name: FOO_PG_WRITES\n    value: "1"\n' > svc/k8s/v2/a.yaml
printf 'BAR_SERVE  serve flags default off by design\n' > ci/env-flags-allowlist.txt
git add -A
run() { python3 "$here/check-env-flags.py" "$tmp" >/dev/null 2>&1; }
run || { echo "FAIL: clean fixture should pass"; exit 1; }
printf 'const x = process.env.BAZ_SOURCE\n' > svc/src/b.ts; git add -A
run && { echo "FAIL: unlisted flag should fail"; exit 1; }
printf 'BAZ_SOURCE  listed\n' >> ci/env-flags-allowlist.txt
run || { echo "FAIL: allowlisted flag should pass"; exit 1; }
printf 'GONE_SERVE  stale\n' >> ci/env-flags-allowlist.txt
run && { echo "FAIL: stale allowlist line should fail"; exit 1; }
printf 'BAR_SERVE  serve flags default off by design\nBAZ_SOURCE  listed\nQUX_PG_WRITES  nope\n' > ci/env-flags-allowlist.txt
run && { echo "FAIL: allowlisted PG_WRITES should fail"; exit 1; }
printf 'BAR_SERVE  serve flags default off by design\nBAZ_SOURCE  listed\n' > ci/env-flags-allowlist.txt
run || { echo "FAIL: baseline before serve fixtures should pass"; exit 1; }
# Serve-gap fixtures: each manifest below would serve users from geo-db and must fail; the safe or marked form must pass.
envf() { printf 'env:\n%s\n' "$1" > svc/k8s/v2/serve.yaml; git add -A; }
expect_fail() { envf "$2"; run && { echo "FAIL: $1 should fail"; exit 1; }; return 0; }
expect_pass() { envf "$2"; run || { echo "FAIL: $1 should pass"; exit 1; }; return 0; }
expect_fail "gateway field on" '  - name: GRAPHQL_GATEWAY_FIELDS
    value: "entity=shadow,entities=on"'
expect_pass "gateway all shadow" '  - name: GRAPHQL_GATEWAY_FIELDS
    value: "entity=shadow,entities=shadow"'
expect_pass "gateway on, marked above" '  # allow-serve: approved cutover of entity
  - name: GRAPHQL_GATEWAY_FIELDS
    value: "entity=on"'
expect_fail "proposal executedAt true" '  - name: PROPOSAL_EXECUTED_AT_FROM_GEODB
    value: "true"'
expect_pass "proposal executedAt false" '  - name: PROPOSAL_EXECUTED_AT_FROM_GEODB
    value: "false"'
expect_fail "vote updates serve" '  - name: NOTIFICATION_INDEXER_GEODB_VOTE_UPDATES
    value: "serve"'
expect_fail "vote updates 1" '  - name: NOTIFICATION_INDEXER_GEODB_VOTE_UPDATES
    value: "1"'
expect_fail "vote updates true" '  - name: NOTIFICATION_INDEXER_GEODB_VOTE_UPDATES
    value: true'
expect_pass "vote updates shadow" '  - name: NOTIFICATION_INDEXER_GEODB_VOTE_UPDATES
    value: "shadow"'
expect_pass "vote updates serve, marked on value" '  - name: NOTIFICATION_INDEXER_GEODB_VOTE_UPDATES
    value: "serve"  # allow-serve: approved'
echo "check-env-flags fixture tests passed"
