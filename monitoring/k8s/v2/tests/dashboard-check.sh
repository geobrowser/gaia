#!/usr/bin/env bash
# Validates monitoring/k8s/v2/geodb-serve-dashboard.yaml without Grafana: the embedded JSON parses, panel ids are
# unique, every panel target has a PromQL expr that promtool parses (as a throwaway recording rule), the geodb_*
# series it selects pass lint-series.py, and the three Plan 10.35 U5 series are each charted. Then proves the
# check bites: a dashboard with a bad expr, a duplicate id or a missing series must fail.
# Needs yq, jq, promtool and python3. Run from anywhere; run.sh calls it.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dash="${here}/../geodb-serve-dashboard.yaml"
work="$(mktemp -d)"
trap 'rm -rf "${work}"' EXIT

required="geodb_store_fold_refused_total geodb_aura_sync_wait_ns_total geodb_store_aura_warm_jobs_total"

# check_json FILE: prints problems, returns non-zero on any.
check_json() {
  local json="$1" ids dup n=0
  jq -e . "${json}" > /dev/null || { echo "dashboard-check FAIL: not valid JSON"; return 1; }
  dup="$(jq -r '[.panels[].id] | group_by(.) | map(select(length > 1) | .[0]) | .[]' "${json}")"
  [ -z "${dup}" ] || { echo "dashboard-check FAIL: duplicate panel id(s): ${dup}"; return 1; }
  jq -r '.panels[] | select(.type != "row" and .type != "text") | (.targets // [])[] | .expr' "${json}" > "${work}/exprs.txt"
  [ -s "${work}/exprs.txt" ] || { echo "dashboard-check FAIL: no panel targets"; return 1; }
  jq -e '[.panels[] | select(.type != "row" and .type != "text") | select((.targets // []) | length == 0)] | length == 0' "${json}" > /dev/null \
    || { echo "dashboard-check FAIL: a panel has no targets"; return 1; }
  {
    echo "groups:"; echo "  - name: dash"; echo "    rules:"
    while IFS= read -r e; do
      n=$((n + 1))
      # Grafana variables are not PromQL: substitute plain values.
      e="${e//\$__rate_interval/5m}"; e="${e//\$__interval/1m}"
      printf '      - record: dash:q%d\n        expr: %s\n' "${n}" "$(jq -Rn --arg e "${e}" '$e')"
    done < "${work}/exprs.txt"
  } > "${work}/dash.rules.yaml"
  promtool check rules "${work}/dash.rules.yaml" > "${work}/promtool.out" 2>&1 \
    || { echo "dashboard-check FAIL: promtool rejected an expr"; cat "${work}/promtool.out"; return 1; }
  jq -R '{"alert": "dashboard", "expr": .}' "${work}/exprs.txt" | jq -s . > "${work}/series.json"
  python3 "${here}/lint-series.py" "${here}/geodb-metrics-golden.txt" "${here}/geodb-label-values.txt" "${work}/series.json" > "${work}/lint.out" \
    || { cat "${work}/lint.out"; return 1; }
  for s in ${required}; do
    grep -q "${s}" "${work}/exprs.txt" || { echo "dashboard-check FAIL: no panel charts ${s}"; return 1; }
  done
  echo "dashboard-check: ${n} exprs ok"
}

yq -r '.data["geodb-serve-u5.json"]' "${dash}" > "${work}/real.json"
check_json "${work}/real.json"

expect_fail() {
  local label="$1" filter="$2"
  jq "${filter}" "${work}/real.json" > "${work}/neg.json"
  if check_json "${work}/neg.json" > "${work}/neg.out" 2>&1; then
    echo "dashboard-check self-test FAIL: ${label} was accepted"; exit 1
  fi
  echo "dashboard-check self-test ok: ${label} rejected"
}
expect_fail "unparsable PromQL" '.panels[2].targets[0].expr = "sum(rate(geodb_aura_sync_wait_ns_total[5m])"'
expect_fail "series not in the golden list" '.panels[2].targets[0].expr = "sum(geodb_no_such_series_total)"'
expect_fail "duplicate panel id" '.panels[3].id = .panels[2].id'
expect_fail "U5 series dropped" '.panels |= map(select(.id != 5))'
