#!/usr/bin/env python3
"""geodb-ingress must not open the wire port (7447) to every namespace.

The policy once allowed `namespaceSelector: {}` on 7447, i.e. any pod in any namespace could
reach geo-db serve. Every real client runs in the gaia namespace, so the rule is pinned to it.
This check reads the manifest as text (standard library only) and fails if any ingress rule that
lists port 7447 has an empty namespaceSelector, an empty podSelector, or no `from` peer at all.
"""
import re
import subprocess
import sys

PATH = "geodb/k8s/v2/geodb.yaml"
text = open(PATH).read()

docs = re.split(r"(?m)^---\s*$", text)
policy = next((d for d in docs if re.search(r"(?m)^kind: NetworkPolicy", d) and re.search(r"(?m)^  name: geodb-ingress\s*$", d)), None)
if policy is None:
    sys.exit(f"{PATH}: NetworkPolicy geodb-ingress not found")

rules = re.split(r"(?m)^    - from:", policy)[1:]
if not rules:
    sys.exit(f"{PATH}: geodb-ingress has no ingress rules with a from list (an empty rule allows everyone)")

seen = False
for rule in rules:
    if not re.search(r"(?m)^\s+- port: 7447\s*$", rule):
        continue
    seen = True
    if re.search(r"(?m)^\s+- (namespaceSelector|podSelector|ipBlock): \{\}\s*$", rule) or "0.0.0.0/0" in rule:
        sys.exit(f"{PATH}: geodb-ingress opens 7447 to every namespace/pod; pin it to the gaia namespace")
    if "kubernetes.io/metadata.name: gaia" not in rule:
        sys.exit(f"{PATH}: geodb-ingress 7447 rule is not pinned to the gaia namespace")
    if not re.search(r"geodb-client:\s*[\"']true[\"']", rule):
        sys.exit(f"{PATH}: geodb-ingress 7447 rule must also select pods with geodb-client: \"true\"")
    if len(re.findall(r"(?m)^\s+- (namespaceSelector|podSelector)\b", rule)) != 1:
        sys.exit(f"{PATH}: geodb-ingress 7447 rule must be ONE peer with both selectors (two peers are OR-ed)")
if not seen:
    sys.exit(f"{PATH}: geodb-ingress has no rule for port 7447")
print("geodb-ingress: 7447 is pinned to gaia pods labelled geodb-client")


# Every workload that talks to geo-db must carry the pod-template label `geodb-client: "true"` so the
# 7447 rule can select it. A workload counts as a client when a non-comment line of its manifest names a
# GEODB_* env var or the geodb:7447 / geodb.gaia address. The geo-db server StatefulSet itself is exempt.
WORKLOADS = {"Deployment", "StatefulSet", "CronJob", "Job", "Pod"}
CLIENT_REF = re.compile(r"GEODB_|geodb:7447|geodb\.gaia")
LABEL = re.compile(r"geodb-client:\s*[\"']true[\"']")
files = subprocess.check_output(
    ["git", "ls-files", "*.yaml", "*.yml"], text=True
).split()
missing = []
clients = 0
for path in files:
    if path.startswith((".github/", "monitoring/")):
        continue
    for doc in re.split(r"(?m)^---\s*$", open(path).read()):
        kind = re.search(r"(?m)^kind: (\w+)\s*$", doc)
        if not kind or kind.group(1) not in WORKLOADS:
            continue
        code = "\n".join(l for l in doc.split("\n") if not l.lstrip().startswith("#"))
        if not CLIENT_REF.search(code):
            continue
        name = re.search(r"(?m)^  name: (\S+)\s*$", doc)
        name = name.group(1) if name else "?"
        if kind.group(1) == "StatefulSet" and name == "geodb":
            continue
        clients += 1
        if kind.group(1) == "Pod":
            region = doc.split("\nspec:", 1)[0]
        else:
            # pod template metadata: from `template:` to the `spec:` that follows it
            m = re.search(r"(?ms)^\s*template:\s*\n(.*?)^\s*spec:\s*$", doc)
            region = m.group(1) if m else ""
        if not LABEL.search(region):
            missing.append(f"{path}: {kind.group(1)} {name}")
if missing:
    sys.exit(
        "workloads that use geo-db are missing the pod label geodb-client: \"true\" "
        "(set it on the pod template, not the selector):\n  " + "\n  ".join(missing)
    )
print(f"geodb-client: all {clients} geo-db client workloads carry the pod label")
