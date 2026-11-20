#!/usr/bin/env python3
"""Preflight gate enforcing the 24 h geodb roll freeze after a read flip or revert.

What is frozen:
  - `geodb-serve` and `geodb-feeder` are always covered (their own rollout is a geodb roll).
  - Any other service (e.g. `api`, `vote-indexer`, `ranking-indexer`, cronjobs) is covered ONLY
    when the `geo/geodb:<tag>` image pin in the manifests about to be applied differs from the
    `geo/geodb:<tag>` pin currently running live on that workload. A routine deploy whose geodb
    image tag is unchanged passes even inside the 24 h window.

Reads the live `geodb-read-source` ConfigMap JSON (passed via `--configmap-json` or stdin)
and checks the newest timestamp across:
  - metadata.annotations["geodb.gaia/last-flip-at"]
  - data["read-source.json"].lastFlipAt
  - data["read-source.json"].reverted[<field>]  (leading ISO-8601 UTC timestamp before space)

Exits 0 if the deploy is not a geodb roll, if no flip/revert occurred within the window, or if a
non-trivial override reason is supplied via `--override`. Fails closed on malformed timestamps or
ConfigMap read errors (`--configmap-read-error`), with a clear message distinguishing read failure
from an active freeze, and allows a named `--override` to pass either.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import sys
from typing import Optional

ALWAYS_FROZEN_SERVICES = frozenset({"geodb-serve", "geodb-feeder"})
ANNOTATION_KEY = "geodb.gaia/last-flip-at"
CONFIGMAP_DATA_KEY = "read-source.json"
TRIVIAL_OVERRIDES = frozenset({"true", "yes", "y", "1", "on", "ok", "force", "override"})
MIN_OVERRIDE_LEN = 8
GEODB_IMAGE_RE = re.compile(r"geo/geodb:([A-Za-z0-9._-]+)")


def parse_iso_utc(raw: str) -> dt.datetime:
    text = raw.strip()
    if text.endswith("Z"):
        text = text[:-1] + "+00:00"
    parsed = dt.datetime.fromisoformat(text)
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=dt.timezone.utc)
    return parsed.astimezone(dt.timezone.utc)


def is_valid_override(raw: str) -> bool:
    reason = raw.strip()
    if len(reason) < MIN_OVERRIDE_LEN:
        return False
    if reason.lower() in TRIVIAL_OVERRIDES:
        return False
    return True


def extract_geodb_tags_from_text(text: str) -> set[str]:
    return set(GEODB_IMAGE_RE.findall(text))


def extract_manifest_tags(paths: list[str]) -> set[str]:
    tags: set[str] = set()
    for p in paths:
        if not p:
            continue
        with open(p, "r", encoding="utf-8") as fh:
            tags.update(extract_geodb_tags_from_text(fh.read()))
    return tags


def extract_live_tags(live_json_paths: list[str], live_tags_arg: Optional[str]) -> set[str]:
    tags: set[str] = set()
    if live_tags_arg:
        for t in live_tags_arg.split(","):
            if t.strip():
                tags.add(t.strip())
    for p in live_json_paths:
        if not p or not os.path.exists(p):
            continue
        with open(p, "r", encoding="utf-8") as fh:
            raw = fh.read().strip()
        if raw:
            tags.update(extract_geodb_tags_from_text(raw))
    return tags


def extract_timestamps(cm: dict) -> list[tuple[dt.datetime, str]]:
    found: list[tuple[dt.datetime, str]] = []

    annotations = (cm.get("metadata") or {}).get("annotations") or {}
    if isinstance(annotations, dict) and ANNOTATION_KEY in annotations:
        val = annotations[ANNOTATION_KEY]
        if isinstance(val, str) and val.strip():
            found.append((parse_iso_utc(val), f"annotation {ANNOTATION_KEY}"))

    data = cm.get("data") or {}
    if isinstance(data, dict) and CONFIGMAP_DATA_KEY in data:
        raw_doc = data[CONFIGMAP_DATA_KEY]
        if isinstance(raw_doc, str) and raw_doc.strip():
            doc = json.loads(raw_doc)
            if not isinstance(doc, dict):
                raise ValueError(f"{CONFIGMAP_DATA_KEY} must be a JSON object")
            last_flip = doc.get("lastFlipAt")
            if isinstance(last_flip, str) and last_flip.strip():
                found.append((parse_iso_utc(last_flip), f"{CONFIGMAP_DATA_KEY}.lastFlipAt"))
            reverted = doc.get("reverted")
            if isinstance(reverted, dict):
                for field, entry in reverted.items():
                    if isinstance(entry, str) and entry.strip():
                        ts_token = entry.strip().split()[0]
                        found.append((parse_iso_utc(ts_token), f"{CONFIGMAP_DATA_KEY}.reverted.{field}"))

    return found


def load_configmap(path: Optional[str]) -> Optional[dict]:
    if path is not None:
        if not os.path.exists(path):
            return None
        with open(path, "r", encoding="utf-8") as fh:
            raw = fh.read().strip()
    else:
        raw = sys.stdin.read().strip()
    if not raw:
        return None
    parsed = json.loads(raw)
    if not isinstance(parsed, dict):
        raise ValueError("ConfigMap JSON must be an object")
    return parsed


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="Enforce 24 h geodb roll freeze after a read flip or revert.")
    parser.add_argument("--service", required=True, help="deploy-v2 service input")
    parser.add_argument("--manifest", action="append", default=[], help="Manifest file(s) about to be applied")
    parser.add_argument("--live-json", action="append", default=[], help="Live workload JSON file(s) from kubectl get")
    parser.add_argument("--live-tags", default=None, help="Comma-separated live geo/geodb tags (for tests)")
    parser.add_argument("--configmap-json", default=None, help="Path to ConfigMap JSON file (omit to read stdin)")
    parser.add_argument("--configmap-read-error", default="", help="Non-empty error text when kubectl get configmap failed")
    parser.add_argument("--now", default=None, help="Override current UTC time (ISO-8601, for tests)")
    parser.add_argument("--window-hours", type=float, default=24.0, help="Freeze window in hours (default 24)")
    parser.add_argument("--override", default="", help="Named reason to override an active freeze or read error")
    args = parser.parse_args(argv)

    # Decide whether this deploy is a geodb roll.
    if args.service in ALWAYS_FROZEN_SERVICES:
        roll_reason = f"service '{args.service}'"
    else:
        manifest_tags = extract_manifest_tags(args.manifest)
        if not manifest_tags:
            print(f"geodb-freeze: '{args.service}' manifests carry no geo/geodb image pin; not subject to freeze.")
            return 0
        live_tags = extract_live_tags(args.live_json, args.live_tags)
        if manifest_tags == live_tags:
            print(
                f"geodb-freeze: '{args.service}' geo/geodb pin unchanged ({', '.join(sorted(manifest_tags))}); "
                f"not subject to freeze."
            )
            return 0
        roll_reason = (
            f"service '{args.service}' geodb pin change "
            f"({', '.join(sorted(live_tags)) or 'none'} -> {', '.join(sorted(manifest_tags))})"
        )

    if args.configmap_read_error.strip():
        err = args.configmap_read_error.strip()
        if is_valid_override(args.override):
            print(
                f"::warning::geodb-freeze: ConfigMap read failed ({err}) overridden for {roll_reason}. "
                f"Reason: {args.override.strip()}"
            )
            return 0
        print(
            f"::error::geodb-freeze: refusing {roll_reason} because reading ConfigMap geodb-read-source failed "
            f"after retries ({err}). Retry the deploy or re-dispatch with a named freeze_override reason "
            f"(>= {MIN_OVERRIDE_LEN} chars).",
            file=sys.stderr,
        )
        return 1

    now = parse_iso_utc(args.now) if args.now else dt.datetime.now(dt.timezone.utc)
    window = dt.timedelta(hours=args.window_hours)

    try:
        cm = load_configmap(args.configmap_json)
        if cm is None:
            print(f"geodb-freeze: ConfigMap geodb-read-source not found or empty; no freeze active for {roll_reason}.")
            return 0
        timestamps = extract_timestamps(cm)
    except Exception as exc:
        if is_valid_override(args.override):
            print(
                f"::warning::geodb-freeze: ConfigMap parse error ({exc}) overridden for {roll_reason}. "
                f"Reason: {args.override.strip()}"
            )
            return 0
        print(
            f"::error::geodb-freeze: refusing {roll_reason} because ConfigMap geodb-read-source could not be parsed "
            f"({exc}). Fix the ConfigMap or supply a named freeze_override reason.",
            file=sys.stderr,
        )
        return 1

    if not timestamps:
        print(f"geodb-freeze: no read flip or revert recorded in geodb-read-source; {roll_reason} may proceed.")
        return 0

    latest_ts, source = max(timestamps, key=lambda item: item[0])
    elapsed = now - latest_ts
    if elapsed >= window:
        hours_since = elapsed.total_seconds() / 3600.0
        print(
            f"geodb-freeze: latest flip/revert ({source} @ {latest_ts.strftime('%Y-%m-%dT%H:%M:%SZ')}) "
            f"was {hours_since:.1f}h ago (>= {args.window_hours:g}h); {roll_reason} may proceed."
        )
        return 0

    remaining = window - elapsed
    rem_hours = remaining.total_seconds() / 3600.0
    if is_valid_override(args.override):
        print(
            f"::warning::geodb-freeze: overriding active freeze for {roll_reason} "
            f"({source} @ {latest_ts.strftime('%Y-%m-%dT%H:%M:%SZ')}, {rem_hours:.1f}h remaining). "
            f"Reason: {args.override.strip()}"
        )
        return 0

    print(
        f"::error::geodb-freeze: refusing {roll_reason} inside the {args.window_hours:g}h freeze window "
        f"after {source} @ {latest_ts.strftime('%Y-%m-%dT%H:%M:%SZ')} ({rem_hours:.1f}h remaining). "
        f"Wait until the window clears or re-dispatch with a named freeze_override reason (>= {MIN_OVERRIDE_LEN} chars).",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
