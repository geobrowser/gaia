#!/usr/bin/env python3
"""geodb-ingress must not open the wire port (7447) to every namespace.

The policy once allowed `namespaceSelector: {}` on 7447, i.e. any pod in any namespace could
reach geo-db serve. Every real client runs in the gaia namespace, so the rule is pinned to it.
This check reads the manifest as text (standard library only) and fails if any ingress rule that
lists port 7447 has an empty namespaceSelector, an empty podSelector, or no `from` peer at all.
"""
import re
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
if not seen:
    sys.exit(f"{PATH}: geodb-ingress has no rule for port 7447")
print("geodb-ingress: 7447 is pinned to the gaia namespace")
