#!/usr/bin/env bash
# Print the newest geo-db `final` commit whose CI passed, using whichever API GH_TOKEN can read.
#
#   1. Actions runs (needs "Actions: read"): newest successful `ci` run on a push to final.
#   2. Check runs (needs "Checks: read"): walk final's newest 30 commits and take the first
#      whose check runs are all complete with no failure.
#
# geo-db posts no commit statuses (its combined status is always "pending"), so that API is no
# fallback.
#
# Exit 0 and print the SHA, or exit 2 with no output when no commit is green yet, or exit 1 when
# the token can read neither (the message names the permissions to add).
# With PROBE=1 it only reports which APIs the token can read and always exits 0.

set -uo pipefail
repo=geobrowser/geo-db

can() { gh api "$1" --silent > /dev/null 2>&1; }

if [ "${PROBE:-0}" = 1 ]; then
  head="$(gh api "repos/$repo/commits/final" --jq .sha 2>/dev/null || true)"
  echo "contents (commits on final): $([ -n "$head" ] && echo yes || echo no)"
  can "repos/$repo/actions/workflows/ci.yml/runs?branch=final&per_page=1" && a=yes || a=no
  echo "actions runs: $a"
  if [ -n "$head" ]; then
    can "repos/$repo/commits/$head/check-runs?per_page=1" && c=yes || c=no
    can "repos/$repo/commits/$head/status" && s=yes || s=no
    echo "check runs: $c"
    echo "commit status: $s"
  fi
  exit 0
fi

if sha="$(gh api "repos/$repo/actions/workflows/ci.yml/runs?branch=final&event=push&status=success&per_page=1" \
      --jq '.workflow_runs[0].head_sha // empty' 2>/dev/null)"; then
  [ -n "$sha" ] || exit 2
  echo "via actions runs" >&2
  echo "$sha"
  exit 0
fi

if ! commits="$(gh api "repos/$repo/commits?sha=final&per_page=30" --jq '.[].sha' 2>/dev/null)"; then
  echo "cannot list geo-db final commits: the token needs \"Contents: read\" on $repo" >&2
  exit 1
fi

for c in $commits; do
  # Complete with nothing failing: success, skipped and neutral all count as passing.
  # shellcheck disable=SC2016 # jq program, not shell
  if ! verdict="$(gh api "repos/$repo/commits/$c/check-runs?per_page=100" --jq '
        [.check_runs[]] as $r
        | if ($r | length) == 0 then "none"
          elif any($r[]; .status != "completed") then "pending"
          elif any($r[]; (.conclusion // "") as $k | ["success","skipped","neutral"] | index($k) | not) then "failed"
          else "green" end' 2>/dev/null)"; then
    echo "the token can read geo-db's commits but not its CI results: add \"Actions: read\" (or \"Checks: read\") on $repo to GEO_DB_READ_TOKEN" >&2
    exit 1
  fi
  if [ "$verdict" = green ]; then echo "via check runs" >&2; echo "$c"; exit 0; fi
done
exit 2
