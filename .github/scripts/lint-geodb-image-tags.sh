#!/usr/bin/env bash
# Fails when any manifest pins the geodb image (registry.digitalocean.com/geo/geodb:<tag>, the
# `geodb-setup` init containers and the Job/CronJob templates included) at a tag other than the
# one the serve container in geodb/k8s/v2/geodb.yaml pins. geodb-feeder and other images are a
# different repository name and never match. Skipped on purpose:
#   - placeholder tags (all caps, e.g. GEODB_TAG): the file is a template that is filled in
#     from the serve's tag at apply time;
#   - geodb/k8s/v2/geodb-cap-raise-pod.yaml: a one-off Pod that runs a specific older build.
# Run from anywhere. With --self-test it also proves the check bites on a synthetic tree.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
serve_file="geodb/k8s/v2/geodb.yaml"
skip_re='^geodb/k8s/v2/geodb-cap-raise-pod\.yaml$'

lint() {
  local dir="$1"
  cd "${dir}"
  local serve
  serve="$(awk '/^ *- name: serve$/{s=1} s && /image: registry.digitalocean.com\/geo\/geodb:/{sub(/.*geodb:/,""); print; exit}' "${serve_file}")"
  if [[ -z "${serve}" ]]; then
    echo "geodb-image-tag lint: no serve image tag found in ${serve_file}"; return 1
  fi
  local rows
  rows="$(grep -rIn --include='*.yaml' --include='*.yml' -E 'image: *"?registry\.digitalocean\.com/geo/geodb:[^ "]+' . \
    | sed -E 's|^\./||; s|^([^:]+):([0-9]+):.*geodb:([^ "]+).*$|\1 \2 \3|' || true)"
  local total=0 bad=0 report=""
  while read -r file line tag; do
    [[ -z "${file}" ]] && continue
    [[ "${file}" =~ ${skip_re} ]] && continue
    [[ "${tag}" =~ ^[A-Z_]+$ ]] && continue
    total=$((total + 1))
    if [[ "${tag}" != "${serve}" ]]; then
      bad=$((bad + 1)); report+="  DRIFT ${file}:${line} geodb:${tag}"$'\n'
    fi
  done <<< "${rows}"
  if (( bad > 0 )); then
    echo "geodb-image-tag lint FAILED: ${bad} of ${total} pinned geodb image tags differ from the serve tag (${serve_file}: ${serve})"
    printf '%s' "${report}"
    echo "Pin every geodb image in these manifests to the same tag as the serve container."
    return 1
  fi
  echo "geodb-image-tag lint ok: ${total} pinned geodb image tags, all ${serve} (serve: ${serve_file})"
}

if [[ "${1:-}" == "--self-test" ]]; then
  lint "${root}"
  work="$(mktemp -d)"; trap 'rm -rf "${work}"' EXIT
  mkdir -p "${work}/geodb/k8s/v2" "${work}/x"
  img() { echo "          image: registry.digitalocean.com/geo/$1"; }
  { echo "      - name: serve"; img geodb:aaaaaaaaaa; } > "${work}/geodb/k8s/v2/geodb.yaml"
  { img geodb:aaaaaaaaaa; img geodb:GEODB_TAG; img geodb-feeder:latest; } > "${work}/x/ok.yaml"
  lint "${work}" > "${work}/ok.out" || { echo "self-test FAIL: clean tree rejected"; cat "${work}/ok.out"; exit 1; }
  img geodb:bbbbbbbbbb > "${work}/x/drift.yaml"
  if lint "${work}" > "${work}/bad.out"; then echo "self-test FAIL: drift accepted"; exit 1; fi
  grep -q 'DRIFT x/drift.yaml:1 geodb:bbbbbbbbbb' "${work}/bad.out" || { echo "self-test FAIL: drift not printed"; cat "${work}/bad.out"; exit 1; }
  echo "geodb-image-tag lint self-test ok: drift rejected and reported"
else
  lint "${root}"
fi
