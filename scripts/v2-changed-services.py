#!/usr/bin/env python3
"""Which gaia v2 services does a range of commits touch?

Used by .github/workflows/deploy-v2-on-merge.yml to decide what to build and deploy after a
push to main, and by its staleness guard to ask "has a newer commit on main touched this
service?" before deploying an older one.

There is deliberately no second list of services or paths to keep in sync. Everything is
read from the two workflows that already ship the stack:

  build-v2-images.yml   the {"image","dockerfile","context"} matrix: what can be built
  deploy-v2.yml         the `<svc>) manifests="..."` case arms: what gets applied

A service's inputs are its Dockerfile, its build context (the whole directory when the
context is not the repo root; otherwise the sources of the Dockerfile's non-stage COPY/ADD
lines), and the manifests deploy-v2 applies for it.

Never selected, whatever changed:
  geodb-serve  the geodb StatefulSet's image is the pin roll, which stays manual
  valkey       pinned third-party image
  any service only one of the two workflows knows (chain-keepalive has no deploy arm)

Usage:
  v2-changed-services.py --base <sha> --head <sha>        JSON list of services touched
  v2-changed-services.py --files <path>...               same, from an explicit file list
  v2-changed-services.py --geodb-pin-changed --base B --head H
                                                          exit 0 if B..H changes a geodb image pin
  v2-changed-services.py --list                           every auto-deployable service and its inputs
  v2-changed-services.py --self-check                     parse sanity checks (CI)
"""

import argparse
import json
import os
import re
import subprocess
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
BUILD_WF = ".github/workflows/build-v2-images.yml"
DEPLOY_WF = ".github/workflows/deploy-v2.yml"

# Pinned images. The geodb serve image changes only through the manual pin roll.
EXCLUDED = {"geodb-serve", "valkey"}

# A line that pins the geodb image (serve, or the geodb-setup init containers in the api and
# feeder manifests). A push that changes one is a pin roll: auto-deploy stays out of it.
GEODB_PIN_RE = re.compile(r"registry\.digitalocean\.com/geo/geodb:[0-9a-f]")

# Changes that cannot alter what a service runs: tests and prose. A push of only these
# builds nothing.
IGNORED_RE = re.compile(r"(^|/)(__tests__|tests)/|\.(test|spec)\.[jt]sx?$|\.md$")

MATRIX_RE = re.compile(
    r'\{"image":"(?P<image>[^"]+)","dockerfile":"(?P<dockerfile>[^"]+)","context":"(?P<context>[^"]+)"\}'
)
DEPLOY_ARM_RE = re.compile(r'^\s*(?P<svc>[a-z0-9-]+)\)\s+manifests="(?P<manifests>[^"]*)"', re.M)


def read(rel):
    with open(os.path.join(ROOT, rel)) as f:
        return f.read()


def norm(path):
    path = os.path.normpath(path)
    return "" if path == "." else path


def copy_sources(dockerfile_rel):
    """Sources of COPY/ADD lines that read from the build context (not `--from=` a stage)."""
    sources = []
    for raw in read(dockerfile_rel).splitlines():
        line = raw.strip()
        m = re.match(r"(?i)^(COPY|ADD)\s+(.*)$", line)
        if not m:
            continue
        args = m.group(2).split()
        if any(a.startswith("--from") for a in args):
            continue
        args = [a for a in args if not a.startswith("--")]
        if len(args) < 2:
            continue
        sources.extend(args[:-1])
    return sources


def services():
    """{service: sorted list of path prefixes whose change means rebuild + redeploy}."""
    build = {m["image"]: m for m in MATRIX_RE.finditer(read(BUILD_WF))}
    deploy = {m["svc"]: m["manifests"].split() for m in DEPLOY_ARM_RE.finditer(read(DEPLOY_WF))}
    out = {}
    for svc in sorted(set(build) & set(deploy) - EXCLUDED):
        b = build[svc]
        ctx = norm(b["context"])
        inputs = {norm(b["dockerfile"])}
        if ctx:
            inputs.add(ctx)
        else:
            for src in copy_sources(b["dockerfile"]):
                inputs.add(norm(src))
        inputs.update(norm(m) for m in deploy[svc])
        inputs.discard("")
        out[svc] = sorted(inputs)
    return out


def touches(path, prefix):
    return path == prefix or path.startswith(prefix + "/")


def select(files, svc_map):
    files = [f for f in files if not IGNORED_RE.search(f)]
    return sorted(s for s, prefixes in svc_map.items() if any(touches(f, p) for f in files for p in prefixes))


def git(*args):
    return subprocess.run(["git", "-C", ROOT, *args], check=True, capture_output=True, text=True).stdout


def changed_files(base, head):
    return [l for l in git("diff", "--name-only", base, head).splitlines() if l]


def geodb_pin_changed(base, head):
    for line in git("diff", "--unified=0", base, head).splitlines():
        if line[:1] in "+-" and not line.startswith(("+++", "---")) and GEODB_PIN_RE.search(line):
            return True
    return False


def self_check():
    svc_map = services()
    problems = []
    # Every service either workflow knows must be auto-deployable, excluded on purpose, or
    # build-only. A new deploy arm without a matching build entry (or the reverse) shows up here.
    build = {m["image"] for m in MATRIX_RE.finditer(read(BUILD_WF))}
    deploy = {m["svc"] for m in DEPLOY_ARM_RE.finditer(read(DEPLOY_WF))}
    if len(build) < 10 or len(deploy) < 10:
        problems.append(f"parsed too few services (build {len(build)}, deploy {len(deploy)}): did a workflow's format change?")
    for svc in sorted(deploy - build - EXCLUDED):
        problems.append(f"{svc}: deploy-v2 has an arm but build-v2-images cannot build it")
    for svc, prefixes in svc_map.items():
        for p in prefixes:
            if not os.path.exists(os.path.join(ROOT, p)):
                problems.append(f"{svc}: input {p} does not exist")
    for svc in EXCLUDED:
        if svc in svc_map:
            problems.append(f"{svc} is excluded but selected")
    # Behavioural checks on the real map.
    cases = [
        (["geodb/k8s/v2/geodb.yaml"], []),
        (["geodb/k8s/v2/geodb-snapshot.yaml"], ["geodb-feeder"]),
        (["geodb/k8s/v2/geodb-card-replay.yaml"], ["geodb-feeder"]),
        (["api/src/index.ts"], ["api"]),
        (["api/k8s/v2/valkey.yaml"], ["api"]),  # the api context is the whole api/ directory
        (["docs/runbooks/deployment.md"], []),
        (["geodb-feeder/src/__tests__/errorTable.test.ts"], []),
        (["kg-indexer/tests/e2e.rs", "kg-indexer/README.md"], []),
        # ranking-indexer copies kg-indexer: its delete_entity_ranks test dev-depends on it.
        (["kg-indexer/src/main.rs"], ["kg-indexer", "ranking-indexer"]),
        (["geodb-cursor/src/lib.rs"], ["hermes-ipfs-cache", "hermes-pipeline", "notification-indexer", "ranking-indexer", "search-indexer", "topology-indexer", "vote-indexer"]),
        (["hermes/k8s/v2/atlas.yaml"], ["atlas"]),
        (["ranking-indexer/k8s/v2/backfill-cronjob.yaml"], ["ranking-indexer"]),
    ]
    for files, want in cases:
        got = select(files, svc_map)
        if got != want:
            problems.append(f"select({files}) = {got}, want {want}")
    if "Cargo.lock" not in svc_map.get("kg-indexer", []):
        problems.append("kg-indexer should rebuild on a Cargo.lock change")
    if not GEODB_PIN_RE.search("          image: registry.digitalocean.com/geo/geodb:99fdac58cf"):
        problems.append("GEODB_PIN_RE does not match a pin line")
    if GEODB_PIN_RE.search("          image: registry.digitalocean.com/geo/geodb-feeder:latest"):
        problems.append("GEODB_PIN_RE matches the geodb-feeder image")
    for p in problems:
        print(f"::error::{p}")
    print(f"{len(svc_map)} auto-deployable services; {len(problems)} problem(s)")
    return 1 if problems else 0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base")
    ap.add_argument("--head")
    ap.add_argument("--files", nargs="*")
    ap.add_argument("--geodb-pin-changed", action="store_true")
    ap.add_argument("--list", action="store_true")
    ap.add_argument("--self-check", action="store_true")
    a = ap.parse_args()

    if a.self_check:
        return self_check()
    if a.list:
        json.dump(services(), sys.stdout, indent=2)
        print()
        return 0
    if a.geodb_pin_changed:
        return 0 if geodb_pin_changed(a.base, a.head) else 1
    files = a.files if a.files is not None else changed_files(a.base, a.head)
    print(json.dumps(select(files, services())))
    return 0


if __name__ == "__main__":
    sys.exit(main())
