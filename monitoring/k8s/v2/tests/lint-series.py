#!/usr/bin/env python3
"""Series lint for the geodb_* names a PrometheusRule selects.

usage: lint-series.py GOLDEN RULES_JSON

RULES_JSON is `yq -o=json '[.spec.groups[].rules[] | {"alert": .alert, "expr": .expr}]'` of the rule file. Every
geodb_<name>{matchers} selector in an expr must name a series in GOLDEN, and every matcher key
must be a label key some GOLDEN row of that series carries. Rules:
  - Scrape labels (namespace, job, pod, instance, service, container, endpoint, prometheus) are
    attached by Prometheus, not printed by serve, so they are not checked.
  - session="" selects rows that have no session label: it needs a GOLDEN row without that key.
  - geodb_outbox_mirror_* and geodb_derived_mirror_* come from gaia's own indexers, not serve, so
    they are skipped explicitly.
  - Only selector matchers are checked, not by()/without() lists or label_replace arguments.
"""
import json
import re
import sys

SCRAPE = {"namespace", "job", "pod", "instance", "service", "container", "endpoint", "prometheus"}
SKIP = re.compile(r"^geodb_(outbox_mirror|derived_mirror)_")
SELECTOR = re.compile(r"\b(geodb_[a-z0-9_]+)\s*(\{[^}]*\})?")
MATCHER = re.compile(r'([A-Za-z_][A-Za-z0-9_]*)\s*(=~|!~|!=|=)\s*"((?:[^"\\]|\\.)*)"')


def load_golden(path):
    rows = {}
    for line in open(path):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        m = re.fullmatch(r"([a-z0-9_]+)(?:\{([^}]*)\})?", line)
        if not m:
            sys.exit(f"{path}: unparsable row: {line}")
        keys = frozenset(k for k in (m.group(2) or "").split(",") if k)
        rows.setdefault(m.group(1), []).append(keys)
    return rows


def check(golden, alert, expr):
    problems = []
    for m in SELECTOR.finditer(expr):
        name, block = m.group(1), m.group(2) or ""
        if SKIP.match(name):
            continue
        if m.start() > 0 and expr[m.start() - 1] == '"':
            continue  # a name inside a __name__=~"..." regex, not a selector
        if name not in golden:
            problems.append(f"{alert}: series {name} is not in the golden list")
            continue
        for key, op, val in MATCHER.findall(block):
            if key in SCRAPE:
                continue
            rows = golden[name]
            if op == "=" and val == "":
                ok = any(key not in r for r in rows)
                why = f"no {name} row without label {key} (selector {key}=\"\")"
            else:
                ok = any(key in r for r in rows)
                why = f"no {name} row carries label {key}"
            if not ok:
                problems.append(f"{alert}: {why}")
    return problems


def main():
    golden = load_golden(sys.argv[1])
    rules = json.load(open(sys.argv[2]))
    problems = []
    checked = 0
    for r in rules:
        if "alert" not in r and "expr" not in r:
            continue
        checked += 1
        problems += check(golden, r.get("alert") or "<record>", str(r["expr"]))
    for p in problems:
        print("series-lint FAIL:", p)
    print(f"series-lint: {checked} rules, {len(problems)} problems")
    sys.exit(1 if problems else 0)


main()
