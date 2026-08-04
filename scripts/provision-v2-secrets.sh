#!/usr/bin/env bash
#
# Provision and verify the v2 cluster's Kubernetes secrets from
# scripts/v2-secrets.spec.yaml.
#
#   ./scripts/provision-v2-secrets.sh              # verify (default, read-only)
#   ./scripts/provision-v2-secrets.sh --apply      # create/merge missing keys
#   ./scripts/provision-v2-secrets.sh --diff       # show what --apply would change
#
# Never prints secret values. Hostname-bearing values are shown with
# credentials stripped, because the whole point is catching a wrong hostname.
#
# Why this exists: every v2 secret incident has been a value copied verbatim
# from the old cluster that needed re-pointing and did not get it — a Valkey URL
# aimed at a namespace that does not exist here, Sentry absent entirely, an
# atlas release tag naming a different service. All were invisible because the
# affected feature was off or unmonitored. `--verify` makes that class loud, and
# is safe to run from CI or a cron.
set -euo pipefail

SPEC="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/v2-secrets.spec.yaml"
MODE="verify"
case "${1:-}" in
	--apply) MODE="apply" ;;
	--diff) MODE="diff" ;;
	--verify | "") MODE="verify" ;;
	-h | --help)
		sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
		exit 0
		;;
	*)
		echo "unknown argument: $1 (try --help)" >&2
		exit 2
		;;
esac

command -v kubectl >/dev/null || { echo "kubectl not found" >&2; exit 1; }
command -v python3 >/dev/null || { echo "python3 not found" >&2; exit 1; }

MODE="$MODE" SPEC="$SPEC" python3 - <<'PYTHON'
import base64, json, os, subprocess, sys

MODE = os.environ["MODE"]
SPEC = os.environ["SPEC"]

# The spec uses a small, fixed subset of YAML; parse it rather than requiring
# PyYAML to be installed on whatever machine is running a rebuild.
def load_spec(path):
    import re
    text = open(path).read()
    # Strip comments and blank lines, then hand-parse the known shape.
    out, cur_list, cur_item, section = {}, None, None, None
    stack = []
    for raw in text.splitlines():
        line = raw.split("#")[0].rstrip() if not raw.strip().startswith("#") else ""
        if not line.strip():
            continue
        indent = len(line) - len(line.lstrip())
        s = line.strip()
        if indent == 0 and s.endswith(":"):
            section = s[:-1]
            out[section] = {} if section in ("target", "source") else []
            continue
        if section in ("target", "source"):
            k, _, v = s.partition(":")
            out[section][k.strip()] = v.strip()
            continue
        if s.startswith("- "):
            cur_item = {}
            out[section].append(cur_item)
            s = s[2:]
        if not s or cur_item is None:
            continue
        k, _, v = s.partition(":")
        k, v = k.strip(), v.strip()
        if v.startswith("[") and v.endswith("]"):
            cur_item[k] = [x.strip() for x in v[1:-1].split(",") if x.strip()]
        elif v in ("", ">-", "|"):
            cur_item[k] = cur_item.get(k, "")
            cur_item.setdefault("_pending", k)
        elif v in ("true", "false"):
            cur_item[k] = v == "true"
        else:
            if cur_item.get("_pending") == k:
                cur_item.pop("_pending", None)
            cur_item[k] = v
        if k == "overrides":
            cur_item[k] = {}
            cur_item["_in_overrides"] = True
    return out

# The hand-parser above cannot handle nested `overrides:` maps or folded
# scalars, so re-read those with a targeted pass.
def load_overrides(path):
    result, name, in_ov = {}, None, False
    for raw in open(path).read().splitlines():
        line = raw.split("#")[0].rstrip() if not raw.strip().startswith("#") else ""
        if not line.strip():
            continue
        s = line.strip()
        if s.startswith("- name:"):
            name, in_ov = s.split(":", 1)[1].strip(), False
            result.setdefault(name, {})
        elif s == "overrides:":
            in_ov = True
        elif in_ov and s.endswith(":"):
            in_ov = False
        elif in_ov and ":" in s and name:
            k, _, v = s.partition(":")
            result[name][k.strip()] = v.strip()
    return result

spec = load_spec(SPEC)
overrides_by_name = load_overrides(SPEC)
TGT_CTX = spec["target"]["context"]
TGT_NS = spec["target"]["namespace"]
SRC_CTX = spec["source"]["context"]

def kget(ctx, ns, name):
    r = subprocess.run(
        ["kubectl", "--context", ctx, "-n", ns, "get", "secret", name, "-o", "json"],
        capture_output=True, text=True)
    return json.loads(r.stdout).get("data", {}) if r.returncode == 0 else None

def redact(value):
    """Keep hosts visible, strip credentials — a wrong host is the bug we hunt."""
    import re
    return re.sub(r"(//)[^:/@]*:[^@]*@", r"\1***:***@", value)

problems, changes = [], []

# ---------------------------------------------------------------------------
# 1. Secrets provisioned from the old cluster
# ---------------------------------------------------------------------------
print(f"target {TGT_CTX}/{TGT_NS}   source {SRC_CTX}\n")
for item in spec.get("secrets", []):
    name = item["name"]
    src_ns, _, src_name = item["from"].partition("/")
    copy_keys = item.get("copy_keys", [])
    ovr = overrides_by_name.get(name, {})
    merge = item.get("merge", False)

    src = kget(SRC_CTX, src_ns, src_name)
    dst = kget(TGT_CTX, TGT_NS, name)

    want = {}
    for k in copy_keys:
        if src is None:
            continue
        if k in src:
            want[k] = src[k]
    for k, v in ovr.items():
        want[k] = base64.b64encode(v.encode()).decode()

    if dst is None and not merge:
        problems.append(f"{name}: secret does not exist on the target")
        changes.append((name, "create", sorted(want)))
        print(f"  MISSING  {name}  (would create with {len(want)} keys)")
        continue
    if dst is None and merge:
        problems.append(f"{name}: expected to exist for merge, but is absent")
        print(f"  MISSING  {name}  (merge target absent — create it first)")
        continue

    missing = [k for k in want if k not in dst]
    wrong = [k for k, v in ovr.items()
             if k in dst and base64.b64decode(dst[k]).decode(errors="replace") != v]
    if missing or wrong:
        for k in missing:
            problems.append(f"{name}/{k}: missing")
        for k in wrong:
            actual = base64.b64decode(dst[k]).decode(errors="replace")
            problems.append(f"{name}/{k}: is '{actual}', spec requires '{ovr[k]}'")
        changes.append((name, "merge", sorted(set(missing) | set(wrong))))
        print(f"  DRIFT    {name}  missing={missing} wrong={wrong}")
    else:
        print(f"  ok       {name}")

# ---------------------------------------------------------------------------
# 2. Invariants — v2-specific values that must never be old-cluster copies
# ---------------------------------------------------------------------------
print("\ninvariants:")
for inv in spec.get("invariants", []):
    name, key, needle = inv["secret"], inv["key"], inv["must_contain"]
    dst = kget(TGT_CTX, TGT_NS, name)
    if dst is None or key not in dst:
        problems.append(f"{name}/{key}: absent, cannot check invariant")
        print(f"  ABSENT   {name}/{key}")
        continue
    val = base64.b64decode(dst[key]).decode(errors="replace")
    if needle in val:
        print(f"  ok       {name}/{key} contains {needle}")
    else:
        problems.append(f"{name}/{key}: expected to contain '{needle}', got '{redact(val)}'")
        print(f"  WRONG    {name}/{key} = {redact(val)}")
        print(f"           expected to contain: {needle}")

# ---------------------------------------------------------------------------
# 3. Manually provisioned secrets — presence only, never contents
# ---------------------------------------------------------------------------
print("\nmanual (not provisioned by this script):")
for m in spec.get("manual", []):
    ns = m.get("namespace", TGT_NS)
    dst = kget(TGT_CTX, ns, m["name"])
    if dst is None:
        problems.append(f"{ns}/{m['name']}: absent (manual provisioning required)")
        print(f"  ABSENT   {ns}/{m['name']}")
    else:
        missing = [k for k in m.get("keys", []) if k not in dst]
        if missing:
            problems.append(f"{ns}/{m['name']}: missing keys {missing}")
            print(f"  PARTIAL  {ns}/{m['name']}  missing={missing}")
        else:
            print(f"  ok       {ns}/{m['name']}")

# ---------------------------------------------------------------------------
# Apply
# ---------------------------------------------------------------------------
if MODE == "apply" and changes:
    print("\napplying:")
    for name, action, keys in changes:
        item = next(i for i in spec["secrets"] if i["name"] == name)
        src_ns, _, src_name = item["from"].partition("/")
        src = kget(SRC_CTX, src_ns, src_name) or {}
        ovr = overrides_by_name.get(name, {})
        data = {k: src[k] for k in item.get("copy_keys", []) if k in src}
        for k, v in ovr.items():
            data[k] = base64.b64encode(v.encode()).decode()
        if action == "create":
            body = {"apiVersion": "v1", "kind": "Secret", "type": "Opaque",
                    "metadata": {"name": name, "namespace": TGT_NS}, "data": data}
            r = subprocess.run(["kubectl", "--context", TGT_CTX, "apply", "-f", "-"],
                               input=json.dumps(body), capture_output=True, text=True)
        else:
            # Merge only the drifted keys. Never replace: these secrets also hold
            # v2-specific values (Kafka, substreams, IPFS) that the source would
            # overwrite with old-cluster ones.
            patch = [{"op": "add", "path": f"/data/{k}", "value": data[k]}
                     for k in keys if k in data]
            r = subprocess.run(["kubectl", "--context", TGT_CTX, "-n", TGT_NS, "patch",
                                "secret", name, "--type=json", "-p", json.dumps(patch)],
                               capture_output=True, text=True)
        print(f"  {action:6} {name}: {'ok' if r.returncode == 0 else r.stderr.strip()[:90]}")
    print("\nWorkloads must be restarted to pick up changed secrets:")
    print("  kubectl --context %s -n %s rollout restart deploy --all" % (TGT_CTX, TGT_NS))

# ---------------------------------------------------------------------------
print()
if problems:
    print(f"{len(problems)} problem(s):")
    for p in problems:
        print(f"  - {p}")
    if MODE == "verify":
        print("\nRun with --apply to fix what this script provisions.")
        print("Anything under `manual` has to be created by hand — see the spec.")
    sys.exit(1)
print("All secrets match the spec.")
PYTHON
