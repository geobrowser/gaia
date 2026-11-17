#!/usr/bin/env bash
# Lints the geodb_* series that monitoring/k8s/v2/geodb-serve-alerts.yaml selects against
# geodb-metrics-golden.txt (a copy of the serve's /metrics names and label keys), then proves the
# lint bites: a rule naming a missing series or a label key the series does not carry must fail.
# Needs yq and python3. Run from anywhere; run.sh calls it.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
rules="${here}/../geodb-serve-alerts.yaml"
golden="${here}/geodb-metrics-golden.txt"
values="${here}/geodb-label-values.txt"
work="$(mktemp -d)"
trap 'rm -rf "${work}"' EXIT

dump() { yq -o=json '[.spec.groups[].rules[] | {"alert": .alert, "expr": .expr}]' "$1" > "$2"; }

dump "${rules}" "${work}/real.json"
python3 "${here}/lint-series.py" "${golden}" "${values}" "${work}/real.json"

# Negative tests: each synthetic rule must be rejected.
expect_fail() {
  local label="$1" expr="$2"
  printf '[{"alert":"T","expr":"%s"}]' "${expr}" > "${work}/neg.json"
  if python3 "${here}/lint-series.py" "${golden}" "${values}" "${work}/neg.json" > "${work}/neg.out"; then
    echo "series-lint self-test FAIL: ${label} was accepted"; cat "${work}/neg.out"; exit 1
  fi
  echo "series-lint self-test ok: ${label} rejected"
}
expect_fail "missing series" 'sum(geodb_no_such_series_total{job=\"geodb\"})'
expect_fail "missing label key" 'sum(geodb_claims_refused_total{nosuch=\"x\"})'
expect_fail "label key on the wrong series" 'sum(geodb_requests_cut_total{claim_class=~\".+\"})'
expect_fail "unknown = value" 'sum(geodb_wire_refused_total{reason=\"revison\"})'
expect_fail "unknown != value" 'geodb_claims_refused_by_cause_total{cause!=\"ceilin\"}'
expect_fail "unknown literal alternative" 'geodb_derived_image_outcome{outcome=~\"loaded|absent|bogus\"}'
expect_fail "value of another series' label" 'geodb_claims_short_total{claim_class=\"construction\"}'
expect_fail "regex matching no value" 'geodb_claims_in_use{claim_class=~\"zz.*\"}'
expect_fail "label with no values row" 'geodb_requests_cut_total{class=\"nope\"}'

# And the positive counterpart: session="" is fine where a fold row exists, mirror series are skipped.
printf '[{"alert":"T","expr":"sum(geodb_requests_cut_total{session=\\"\\"}) + geodb_outbox_mirror_x{a=\\"b\\"}"}]' > "${work}/pos.json"
python3 "${here}/lint-series.py" "${golden}" "${values}" "${work}/pos.json" > /dev/null
echo "series-lint self-test ok: fold row and mirror skip accepted"
cat > "${work}/pos2.json" <<'JSON'
[{"alert":"T","expr":"geodb_derived_image_outcome{outcome!~\"loaded|off\"} + geodb_wire_refused_total{reason=~\".+\"} + geodb_claims_in_use{claim_class=~\"query_.*\"}"}]
JSON
python3 "${here}/lint-series.py" "${golden}" "${values}" "${work}/pos2.json" > /dev/null
echo "series-lint self-test ok: known values, .+ and a matching regex accepted"
