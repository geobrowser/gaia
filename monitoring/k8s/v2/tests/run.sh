#!/usr/bin/env bash
# Runs promtool over every PrometheusRule that has a tests/<name>.test.yaml next to it.
# promtool reads plain rule files, so the PrometheusRule's spec is extracted with yq first.
# Finally lint-series.sh checks the geodb_* series, their label keys and the label values geodb-serve-alerts.yaml
# selects against the golden copy (geodb-metrics-golden.txt) and the pinned values (geodb-label-values.txt).
# Needs promtool (brew install prometheus) and yq. Run from anywhere.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "${work}"' EXIT
status=0
for t in "${here}"/*.test.yaml; do
  name="$(basename "${t}" .test.yaml)"
  yq '.spec' "${here}/../${name}.yaml" > "${work}/${name}.rules.yaml"
  sed "s#__RULES__#${work}/${name}.rules.yaml#" "${t}" > "${work}/${name}.test.yaml"
  promtool check rules "${work}/${name}.rules.yaml"
  promtool test rules "${work}/${name}.test.yaml" || status=1
done
"${here}/lint-series.sh" || status=1
exit "${status}"
