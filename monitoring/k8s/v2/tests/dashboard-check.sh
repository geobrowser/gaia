#!/usr/bin/env bash
# Validates the Grafana dashboard ConfigMaps under monitoring/k8s/v2/ without Grafana:
#   - geodb-serve-dashboard.yaml (geodb-serve-u5.json)
#   - geodb-relations-shadow-dashboard.yaml (geodb-relations-shadow.json)
# Checks that each embedded JSON parses, panel ids are unique, every non-row/non-text panel has a PromQL
# expr that promtool parses (as a throwaway recording rule), any geodb_* series pass lint-series.py, and
# every required series is charted. Then proves the check bites on negative cases.
# Needs yq, jq, promtool and python3. Run from anywhere; run.sh calls it.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
serve_dash="${here}/../geodb-serve-dashboard.yaml"
relations_dash="${here}/../geodb-relations-shadow-dashboard.yaml"
work="$(mktemp -d)"
trap 'rm -rf "${work}"' EXIT

serve_required="geodb_store_fold_refused_total geodb_aura_sync_wait_ns_total geodb_store_aura_warm_jobs_total geodb_store_fenced geodb_wire_refused_total kube_statefulset_status_replicas_ready kube_pod_container_status_restarts_total"
relations_required="gaia_api_gateway_shadow_total gaia_api_gateway_shadow_unsupported_total gaia_api_gateway_shadow_failed_total gaia_api_gateway_shadow_mismatch_total gaia_api_gateway_shadow_geodb_seconds_bucket gaia_api_gateway_shadow_postgres_seconds_bucket relationsConnection"

# check_json FILE REQUIRED_TOKENS: prints problems, returns non-zero on any.
check_json() {
  local json="$1" required="$2" dup n=0
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
  echo "dashboard-check ($(basename "${json}")): ${n} exprs ok"
}

yq -r '.data["geodb-serve-u5.json"]' "${serve_dash}" > "${work}/geodb-serve-u5.json"
check_json "${work}/geodb-serve-u5.json" "${serve_required}"

yq -r '.data["geodb-relations-shadow.json"]' "${relations_dash}" > "${work}/geodb-relations-shadow.json"
check_json "${work}/geodb-relations-shadow.json" "${relations_required}"

expect_fail() {
  local src="$1" required="$2" label="$3" filter="$4"
  jq "${filter}" "${src}" > "${work}/neg.json"
  if check_json "${work}/neg.json" "${required}" > "${work}/neg.out" 2>&1; then
    echo "dashboard-check self-test FAIL: ${label} was accepted"; exit 1
  fi
  echo "dashboard-check self-test ok: ${label} rejected"
}
expect_fail "${work}/geodb-serve-u5.json" "${serve_required}" "unparsable PromQL" '.panels[2].targets[0].expr = "sum(rate(geodb_aura_sync_wait_ns_total[5m])"'
expect_fail "${work}/geodb-serve-u5.json" "${serve_required}" "series not in the golden list" '.panels[2].targets[0].expr = "sum(geodb_no_such_series_total)"'
expect_fail "${work}/geodb-serve-u5.json" "${serve_required}" "duplicate panel id" '.panels[3].id = .panels[2].id'
expect_fail "${work}/geodb-serve-u5.json" "${serve_required}" "U5 series dropped" '.panels |= map(select(.id != 5 and .id != 14))'
expect_fail "${work}/geodb-serve-u5.json" "${serve_required}" "post-roll fenced panels dropped" '.panels |= map(select(.id != 7 and .id != 13))'
expect_fail "${work}/geodb-relations-shadow.json" "${relations_required}" "relations shadow unparsable PromQL" '.panels[1].targets[0].expr = "sum(rate(gaia_api_gateway_shadow_total[5m])"'
expect_fail "${work}/geodb-relations-shadow.json" "${relations_required}" "relations shadow latency panel dropped" '.panels |= map(select(.id != 5 and .id != 9))'
