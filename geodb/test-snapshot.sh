#!/usr/bin/env bash
# Deterministic stub-kubectl tests for the snapshot scripts embedded in geodb/k8s/v2/geodb-snapshot.yaml.
# Offline: no cluster, no DigitalOcean API.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
manifest="$here/k8s/v2/geodb-snapshot.yaml"

fail=0
passed=0
check() {
  if [[ "$2" == "$3" ]]; then
    echo "ok   $1"
    passed=$((passed + 1))
  else
    echo "FAIL $1"
    echo "  want: $3"
    echo "  got:  $2"
    fail=1
  fi
}

check_contains() {
  local name="$1" haystack="$2" needle="$3"
  if grep -Fq -- "$needle" <<<"$haystack"; then
    echo "ok   $name"
    passed=$((passed + 1))
  else
    echo "FAIL $name"
    echo "  missing substring: $needle"
    echo "  output:"
    printf '%s\n' "$haystack" | sed 's/^/    /'
    fail=1
  fi
}

check_not_contains() {
  local name="$1" haystack="$2" needle="$3"
  if grep -Fq -- "$needle" <<<"$haystack"; then
    echo "FAIL $name"
    echo "  unexpected substring: $needle"
    fail=1
  else
    echo "ok   $name"
    passed=$((passed + 1))
  fi
}

# Extract the inline bash script from a named CronJob in geodb/k8s/v2/geodb-snapshot.yaml.
extract_script() {
  local target="$1"
  awk -v target="$target" '
    /^---/ { in_cj=0; matched=0; in_args=0; in_script=0; next }
    /^kind: CronJob$/ { in_cj=1; next }
    in_cj && /^  name: / {
      name=$2
      if (name == target) matched=1
      next
    }
    matched && /^              args:$/ { in_args=1; next }
    in_args && /^                - \|$/ { in_script=1; in_args=0; next }
    in_script {
      if ($0 ~ /^                  /) {
        sub(/^                  /, "")
        print
      } else if ($0 ~ /^$/) {
        print ""
      } else {
        in_script=0
      }
    }
  ' "$manifest"
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

daily_script="$tmp/geodb-snapshot.sh"
fourh_script="$tmp/geodb-snapshot-4h.sh"
extract_script geodb-snapshot >"$daily_script"
extract_script geodb-snapshot-4h >"$fourh_script"

# 0. Both CronJobs extract non-empty, identical parameterized scripts.
check "daily script extracted" "$([[ -s "$daily_script" ]] && echo yes || echo no)" "yes"
check "4h script extracted" "$([[ -s "$fourh_script" ]] && echo yes || echo no)" "yes"
if diff -u "$daily_script" "$fourh_script" >/dev/null 2>&1; then
  check "daily and 4h scripts identical" same same
else
  check "daily and 4h scripts identical" different same
fi

if command -v shellcheck >/dev/null 2>&1; then
  if shellcheck -s bash "$daily_script" >/dev/null 2>&1; then
    check "shellcheck on extracted script" clean clean
  else
    echo "FAIL shellcheck on extracted script"
    shellcheck -s bash "$daily_script" || true
    fail=1
  fi
fi

# Create stub kubectl in $tmp/bin/kubectl backed by $STUB_STATE_DIR/snapshots.tsv.
# TSV columns: pvc<TAB>name<TAB>app<TAB>snapshot-kind<TAB>snapshot-stamp
mkdir -p "$tmp/bin"
cat >"$tmp/bin/kubectl" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
state_file="${STUB_STATE_DIR}/snapshots.tsv"
log_file="${STUB_STATE_DIR}/kubectl.log"
printf '%s\n' "$*" >>"$log_file"

cmd="${1:-}"
shift || true

case "$cmd" in
  get)
    res="${1:-}"
    shift || true
    case "$res" in
      pvc)
        printf 'Bound'
        ;;
      volumesnapshot)
        sel=""
        jsonpath=""
        while [[ $# -gt 0 ]]; do
          case "$1" in
            -l) sel="$2"; shift 2 ;;
            -o)
              case "$2" in
                jsonpath=*) jsonpath="${2#jsonpath=}" ;;
              esac
              shift 2
              ;;
            --sort-by=*) shift ;;
            *) shift ;;
          esac
        done

        req_app=""
        req_kind=""
        req_stamp=""
        if [[ -n "$sel" ]]; then
          IFS=',' read -ra parts <<<"$sel"
          for p in "${parts[@]}"; do
            k="${p%%=*}"
            v="${p#*=}"
            case "$k" in
              app) req_app="$v" ;;
              snapshot-kind) req_kind="$v" ;;
              snapshot-stamp) req_stamp="$v" ;;
            esac
          done
        fi

        while IFS=$'\t' read -r pvc name app kind stamp; do
          [[ -z "$pvc" ]] && continue
          [[ -n "$req_app" && "$app" != "$req_app" ]] && continue
          [[ -n "$req_kind" && "$kind" != "$req_kind" ]] && continue
          [[ -n "$req_stamp" && "$stamp" != "$req_stamp" ]] && continue
          if [[ "$jsonpath" == *"persistentVolumeClaimName"* ]]; then
            printf '%s|%s|%s|%s\n' "$pvc" "$name" "$app" "$kind"
          elif [[ "$jsonpath" == *"snapshot-stamp"* ]]; then
            printf '%s\n' "$stamp"
          else
            printf '%s\n' "$name"
          fi
        done <"$state_file"
        ;;
      *)
        echo "stub kubectl: unexpected get $res" >&2
        exit 9
        ;;
    esac
    ;;
  create)
    # Expect: create -f -
    yaml="$(cat)"
    name="$(awk '/^  name:/ {print $2; exit}' <<<"$yaml" | tr -d '"')"
    app="$(awk '/^    app:/ {print $2; exit}' <<<"$yaml" | tr -d '"')"
    kind="$(awk '/^    snapshot-kind:/ {print $2; exit}' <<<"$yaml" | tr -d '"')"
    stamp="$(awk '/^    snapshot-stamp:/ {print $2; exit}' <<<"$yaml" | tr -d '"')"
    pvc="$(awk '/^    persistentVolumeClaimName:/ {print $2; exit}' <<<"$yaml" | tr -d '"')"
    printf '%s\t%s\t%s\t%s\t%s\n' "$pvc" "$name" "$app" "$kind" "$stamp" >>"$state_file"
    printf 'volumesnapshot.snapshot.storage.k8s.io/%s created\n' "$name"
    ;;
  wait)
    target="${1:-}"
    name="${target#volumesnapshot/}"
    if [[ "${STUB_STATE_WAIT_FAIL:-0}" == "1" && "$name" == *-geodb-feeder-state ]]; then
      echo "error: timed out waiting for the condition on volumesnapshots/${name}" >&2
      exit 1
    fi
    if [[ "${STUB_DATA_WAIT_FAIL:-0}" == "1" && "$name" == *-data-geodb-0 ]]; then
      echo "error: timed out waiting for the condition on volumesnapshots/${name}" >&2
      exit 1
    fi
    printf 'volumesnapshot.snapshot.storage.k8s.io/%s condition met\n' "$name"
    ;;
  delete)
    res="${1:-}"
    shift || true
    [[ "$res" == "volumesnapshot" ]] || { echo "stub kubectl: unexpected delete $res" >&2; exit 9; }
    sel=""
    del_name=""
    while [[ $# -gt 0 ]]; do
      case "$1" in
        -l) sel="$2"; shift 2 ;;
        --ignore-not-found|--wait=false) shift ;;
        *) del_name="${1#volumesnapshot/}"; shift ;;
      esac
    done
    req_app=""
    req_kind=""
    req_stamp=""
    if [[ -n "$sel" ]]; then
      IFS=',' read -ra parts <<<"$sel"
      for p in "${parts[@]}"; do
        k="${p%%=*}"
        v="${p#*=}"
        case "$k" in
          app) req_app="$v" ;;
          snapshot-kind) req_kind="$v" ;;
          snapshot-stamp) req_stamp="$v" ;;
        esac
      done
    fi
    tmp_out="${state_file}.tmp"
    : >"$tmp_out"
    while IFS=$'\t' read -r pvc name app kind stamp; do
      [[ -z "$pvc" ]] && continue
      if [[ -n "$del_name" && "$name" == "$del_name" ]]; then
        continue
      fi
      if [[ -n "$sel" ]]; then
        match=1
        [[ -n "$req_app" && "$app" != "$req_app" ]] && match=0
        [[ -n "$req_kind" && "$kind" != "$req_kind" ]] && match=0
        [[ -n "$req_stamp" && "$stamp" != "$req_stamp" ]] && match=0
        [[ "$match" == 1 ]] && continue
      fi
      printf '%s\t%s\t%s\t%s\t%s\n' "$pvc" "$name" "$app" "$kind" "$stamp" >>"$tmp_out"
    done <"$state_file"
    mv "$tmp_out" "$state_file"
    ;;
  *)
    echo "stub kubectl: unexpected command $cmd $*" >&2
    exit 9
    ;;
esac
STUB
chmod +x "$tmp/bin/kubectl"

# Helper to seed scheduled snapshot rows for a PVC.
seed_sched() {
  local file="$1" pvc="$2" prefix="$3" kind="$4" count="$5"
  local i stamp
  for ((i = 1; i <= count; i++)); do
    stamp=$(printf '202611%02dt091700z' "$i")
    printf '%s\t%s-%s-%s\tgeodb-snapshot\t%s\t%s\n' "$pvc" "$prefix" "$stamp" "$pvc" "$kind" "$stamp" >>"$file"
  done
}

seed_manual() {
  local file="$1" pvc="$2" name="$3"
  printf '%s\t%s\t\t\t\n' "$pvc" "$name" >>"$file"
}

run_case() {
  local script="$1" state_dir="$2"
  shift 2
  env \
    PATH="$tmp/bin:$PATH" \
    STUB_STATE_DIR="$state_dir" \
    NAMESPACE="gaia" \
    READY_TIMEOUT="600s" \
    SNAPSHOT_CAP="25" \
    "$@" \
    bash -eu "$script"
}

# --- Case 1: Both PVCs below cap -> both snapshots taken & pruned cleanly, exit 0 ---
c1="$tmp/case1"
mkdir -p "$c1"
: >"$c1/snapshots.tsv"
seed_sched "$c1/snapshots.tsv" geodb-feeder-state sched scheduled 7
seed_sched "$c1/snapshots.tsv" data-geodb-0 sched scheduled 7
seed_manual "$c1/snapshots.tsv" geodb-feeder-state pretimes-20261105-geodb-feeder-state
seed_manual "$c1/snapshots.tsv" geodb-feeder-state posttimes-20261108-geodb-feeder-state
seed_manual "$c1/snapshots.tsv" data-geodb-0 pretimes-20261105-data-geodb-0

rc=0
out="$(run_case "$daily_script" "$c1" KEEP=7 SNAPSHOT_KIND=scheduled NAME_PREFIX=sched 2>&1)" || rc=$?
check "case1 exit 0" "$rc" "0"
check_contains "case1 feeder count diag" "$out" "pvc=geodb-feeder-state total=9/25 scheduled=7 manual=2"
check_contains "case1 feeder manual names" "$out" "pretimes-20261105-geodb-feeder-state"
check_contains "case1 data count diag" "$out" "pvc=data-geodb-0 total=8/25 scheduled=7 manual=1"
check_contains "case1 data manual names" "$out" "pretimes-20261105-data-geodb-0"
check_contains "case1 ok pair" "$out" "OK: pair "
# 7 scheduled kept + 2 manual on feeder = 9; 7 scheduled kept + 1 manual on data = 8
check "case1 feeder snapshots after prune" "$(grep -c '^geodb-feeder-state	' "$c1/snapshots.tsv")" "9"
check "case1 data snapshots after prune" "$(grep -c '^data-geodb-0	' "$c1/snapshots.tsv")" "8"
check "case1 oldest pair pruned" "$(grep -c '20261101t091700z' "$c1/snapshots.tsv" || true)" "0"
check "case1 manual snapshots preserved" "$(grep -c 'pretimes-20261105' "$c1/snapshots.tsv")" "2"

# --- Case 2a: geodb-feeder-state at 24/25 and times out -> prints diagnostic, STILL takes data-geodb-0, exits non-zero ---
c2a="$tmp/case2a"
mkdir -p "$c2a"
: >"$c2a/snapshots.tsv"
seed_sched "$c2a/snapshots.tsv" geodb-feeder-state sched scheduled 9
seed_sched "$c2a/snapshots.tsv" geodb-feeder-state sched4h scheduled-4h 6
for i in 1 2 3 4 5 6 7 8 9; do
  seed_manual "$c2a/snapshots.tsv" geodb-feeder-state "pretimes-2026110${i}-geodb-feeder-state"
done
seed_sched "$c2a/snapshots.tsv" data-geodb-0 sched scheduled 9
seed_sched "$c2a/snapshots.tsv" data-geodb-0 sched4h scheduled-4h 6

rc=0
out="$(run_case "$fourh_script" "$c2a" KEEP=6 SNAPSHOT_KIND=scheduled-4h NAME_PREFIX=sched4h STUB_STATE_WAIT_FAIL=1 2>&1)" || rc=$?
check "case2a exit non-zero" "$([[ $rc -ne 0 ]] && echo nonzero || echo zero)" "nonzero"
check_contains "case2a feeder count diag" "$out" "pvc=geodb-feeder-state total=24/25 scheduled=15 manual=9"
check_contains "case2a feeder manual name" "$out" "pretimes-20261101-geodb-feeder-state"
check_contains "case2a data count diag" "$out" "pvc=data-geodb-0 total=15/25 scheduled=15 manual=0"
# Failed feeder snapshot was removed (still 24), clean data-geodb-0 snapshot was preserved (15 -> 16)
check "case2a feeder count unchanged (failed snap removed)" "$(grep -c '^geodb-feeder-state	' "$c2a/snapshots.tsv")" "24"
check "case2a data snapshot taken and preserved" "$(grep -c '^data-geodb-0	' "$c2a/snapshots.tsv")" "16"
check_not_contains "case2a retention skipped" "$out" "pruning pair"

# --- Case 2b: geodb-feeder-state already at 25/25 cap -> fast-fails feeder, STILL takes data-geodb-0, exits non-zero ---
c2b="$tmp/case2b"
mkdir -p "$c2b"
: >"$c2b/snapshots.tsv"
seed_sched "$c2b/snapshots.tsv" geodb-feeder-state sched scheduled 10
seed_sched "$c2b/snapshots.tsv" geodb-feeder-state sched4h scheduled-4h 6
for i in 1 2 3 4 5 6 7 8 9; do
  seed_manual "$c2b/snapshots.tsv" geodb-feeder-state "posttimes-2026110${i}-geodb-feeder-state"
done
seed_sched "$c2b/snapshots.tsv" data-geodb-0 sched scheduled 9
seed_sched "$c2b/snapshots.tsv" data-geodb-0 sched4h scheduled-4h 6

rc=0
out="$(run_case "$fourh_script" "$c2b" KEEP=6 SNAPSHOT_KIND=scheduled-4h NAME_PREFIX=sched4h 2>&1)" || rc=$?
check "case2b exit non-zero" "$([[ $rc -ne 0 ]] && echo nonzero || echo zero)" "nonzero"
check_contains "case2b feeder cap diag" "$out" "pvc=geodb-feeder-state total=25/25 scheduled=16 manual=9"
check_contains "case2b feeder manual name" "$out" "posttimes-20261109-geodb-feeder-state"
check_contains "case2b feeder cap skip msg" "$out" "at/over cap (25/25)"
check "case2b feeder count unchanged" "$(grep -c '^geodb-feeder-state	' "$c2b/snapshots.tsv")" "25"
check "case2b data snapshot taken and preserved" "$(grep -c '^data-geodb-0	' "$c2b/snapshots.tsv")" "16"
check_not_contains "case2b retention skipped" "$out" "pruning pair"

# --- Case 3: data-geodb-0 at 25/25 cap -> prints exact per-PVC cap diagnostic, cleans unpaired feeder snap, exits non-zero ---
c3="$tmp/case3"
mkdir -p "$c3"
: >"$c3/snapshots.tsv"
seed_sched "$c3/snapshots.tsv" geodb-feeder-state sched scheduled 10
seed_sched "$c3/snapshots.tsv" data-geodb-0 sched scheduled 10
seed_sched "$c3/snapshots.tsv" data-geodb-0 sched4h scheduled-4h 6
for i in 1 2 3 4 5 6 7 8 9; do
  seed_manual "$c3/snapshots.tsv" data-geodb-0 "premigrate-2026110${i}-data-geodb-0"
done

rc=0
out="$(run_case "$fourh_script" "$c3" KEEP=6 SNAPSHOT_KIND=scheduled-4h NAME_PREFIX=sched4h 2>&1)" || rc=$?
check "case3 exit non-zero" "$([[ $rc -ne 0 ]] && echo nonzero || echo zero)" "nonzero"
check_contains "case3 feeder count diag" "$out" "pvc=geodb-feeder-state total=10/25 scheduled=10 manual=0"
check_contains "case3 data cap diag" "$out" "pvc=data-geodb-0 total=25/25 scheduled=16 manual=9"
check_contains "case3 data manual name" "$out" "premigrate-20261101-data-geodb-0"
check_contains "case3 data cap skip msg" "$out" "at/over cap (25/25)"
check "case3 data count unchanged" "$(grep -c '^data-geodb-0	' "$c3/snapshots.tsv")" "25"
check "case3 unpaired feeder snapshot cleaned up" "$(grep -c '^geodb-feeder-state	' "$c3/snapshots.tsv")" "10"
check_not_contains "case3 retention skipped" "$out" "pruning pair"

# --- Case 4: data-geodb-0 below cap but times out -> cleans up both, exits non-zero ---
c4="$tmp/case4"
mkdir -p "$c4"
: >"$c4/snapshots.tsv"
seed_sched "$c4/snapshots.tsv" geodb-feeder-state sched scheduled 10
seed_sched "$c4/snapshots.tsv" data-geodb-0 sched scheduled 15

rc=0
out="$(run_case "$fourh_script" "$c4" KEEP=6 SNAPSHOT_KIND=scheduled-4h NAME_PREFIX=sched4h STUB_DATA_WAIT_FAIL=1 2>&1)" || rc=$?
check "case4 exit non-zero" "$([[ $rc -ne 0 ]] && echo nonzero || echo zero)" "nonzero"
check "case4 feeder unpaired snap cleaned up" "$(grep -c '^geodb-feeder-state	' "$c4/snapshots.tsv")" "10"
check "case4 failed data snap cleaned up" "$(grep -c '^data-geodb-0	' "$c4/snapshots.tsv")" "15"
check_not_contains "case4 retention skipped" "$out" "pruning pair"

echo "summary: ${passed} passed, fail=${fail}"
exit "$fail"
