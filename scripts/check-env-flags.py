#!/usr/bin/env python3
"""Every flag-shaped env name read in code must be set in a v2 manifest or be allowlisted.

An unset flag reads as its code default. For a *_PG_WRITES switch that default is "on", so a writer
that is in no manifest cannot be turned off by editing one (gaia#1536 fixed eight by hand). This
script collects every env name matching GEODB_*, *_PG_WRITES, *_SOURCE, *_SERVE or
DELIVERY_WORKER_CLAIM_* (which also covers SEARCH_INDEXER_GEODB_*) that code reads, and fails unless
the name appears in a manifest under any v2 directory (k8s/v2 or deployment/v2) or in ci/env-flags-allowlist.txt.

A name counts as read when it is
  - a quoted string literal in a .rs, .ts, .tsx or .py file (covers env::var("X"), get("X"),
    os.environ["X"], os.getenv("X") and the helper wrappers around them), or
  - accessed as process.env.X, Bun.env.X or env.X (the feeder and api pass `env` around).
Allowlist lines are `NAME  reason` (one reason per line, `#` starts a comment). An allowlisted name
that is no longer read anywhere, or is now in a manifest, also fails, so the list cannot rot.

Usage: check-env-flags.py [repo_root]   (standard library only)
"""
import os
import re
import subprocess
import sys

root = sys.argv[1] if len(sys.argv) > 1 else "."
NAME = r"(?:GEODB_[A-Z0-9_]+|[A-Z0-9_]+_PG_WRITES|[A-Z0-9_]+_SOURCE|[A-Z0-9_]+_SERVE|DELIVERY_WORKER_CLAIM_[A-Z0-9_]+)"
LITERAL = re.compile(r"""["'](%s)["']""" % NAME)
ACCESS = re.compile(r"\benv\.(%s)\b" % NAME)
IN_MANIFEST = re.compile(r"\b(%s)\b" % NAME)


def tracked():
    out = subprocess.run(["git", "-C", root, "ls-files", "-z"], capture_output=True, check=True).stdout
    return [p for p in out.decode().split("\0") if p]


files = tracked()
code, manifests = {}, set()
for rel in files:
    path = os.path.join(root, rel)
    if not os.path.isfile(path):
        continue
    is_manifest = "/v2/" in "/" + rel and rel.endswith((".yaml", ".yml"))
    is_code = rel.endswith((".rs", ".ts", ".tsx", ".py"))
    if not (is_manifest or is_code):
        continue
    try:
        text = open(path, encoding="utf-8").read()
    except (UnicodeDecodeError, OSError):
        continue
    if is_manifest:
        manifests.update(IN_MANIFEST.findall(text))
    else:
        for n in LITERAL.findall(text) + ACCESS.findall(text):
            code.setdefault(n, rel)

allow = {}
allow_path = os.path.join(root, "ci/env-flags-allowlist.txt")
for i, line in enumerate(open(allow_path, encoding="utf-8"), 1):
    line = line.split("#", 1)[0].strip()
    if not line:
        continue
    parts = line.split(None, 1)
    if len(parts) < 2 or not re.fullmatch(NAME, parts[0]):
        sys.exit(f"{allow_path}:{i}: expected `NAME reason`, got: {line!r}")
    if parts[0].endswith("_PG_WRITES"):
        sys.exit(f"{allow_path}:{i}: {parts[0]} cannot be allowlisted; an unset writer switch reads as on, set it in the manifest")
    allow[parts[0]] = parts[1]

missing = sorted(n for n in code if n not in manifests and n not in allow)
stale_gone = sorted(n for n in allow if n not in code)
stale_set = sorted(n for n in allow if n in manifests)
print(f"scanned {len(code)} names, {len(code) - len(missing)} covered "
      f"({len([n for n in code if n in manifests])} in manifests, {len([n for n in code if n in allow and n not in manifests])} allowlisted)")
bad = False
if missing:
    bad = True
    print("\nRead in code but in no v2 manifest and not in ci/env-flags-allowlist.txt:", file=sys.stderr)
    for n in missing:
        print(f"  {n}   (first seen in {code[n]})", file=sys.stderr)
    print("\nSet it explicitly in the manifest of the workload that reads it, or add `NAME reason` to the allowlist.", file=sys.stderr)
if stale_gone:
    bad = True
    print("\nAllowlisted but no longer read in code (remove the line):", *stale_gone, sep="\n  ", file=sys.stderr)
if stale_set:
    bad = True
    print("\nAllowlisted but now set in a manifest (remove the line):", *stale_set, sep="\n  ", file=sys.stderr)
sys.exit(1 if bad else 0)
