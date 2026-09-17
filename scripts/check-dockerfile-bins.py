#!/usr/bin/env python3
"""Every binary a crate builds must be copied into its runtime image.

The runtime stage of each service Dockerfile copies binaries one COPY line at a time.
Adding a binary without adding its line produces an image that builds, pushes, deploys,
shows a healthy workload in kubectl, and whose container can never start:

    exec: "<name>": executable file not found in $PATH

That is not hypothetical. `feed_composition_sample` shipped exactly that way on
2026-09-17: green CI, successful build, successful deploy, a CronJob visible in the
cluster, and a container that died instantly. Every check upstream was green because
nothing looked at Cargo.toml and the Dockerfile together.

WHY THIS IS A WORKSPACE SCRIPT and not a test inside one crate. The first version of this
check was a Rust test in ranking-indexer that parsed `[[bin]]` entries. It would not have
caught the same mistake in kg-indexer, which declares NO `[[bin]]` sections at all — cargo
auto-discovers `src/bin/*.rs`, and kg-indexer ships six binaries that way. A per-crate test
also only protects the crate that remembers to have one.

A binary that is deliberately not shipped is opted out with a line in its Dockerfile:

    # dockerfile-bins: ignore <name>
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def declared_bins(crate: Path) -> set[str]:
    """Every binary cargo will build: explicit [[bin]], auto-discovered src/bin/*.rs, and
    the default bin named after the package when src/main.rs exists."""
    manifest = (crate / "Cargo.toml").read_text()
    bins: set[str] = set()

    in_bin = False
    package_name = None
    in_package = False
    for line in manifest.splitlines():
        stripped = line.strip()
        if stripped.startswith("["):
            in_bin = stripped == "[[bin]]"
            in_package = stripped == "[package]"
            continue
        if in_bin and stripped.startswith("name"):
            bins.add(stripped.split("=", 1)[1].strip().strip('"'))
        if in_package and stripped.startswith("name") and package_name is None:
            package_name = stripped.split("=", 1)[1].strip().strip('"')

    # autobins is on by default for edition 2018+; these need no manifest entry.
    bin_dir = crate / "src" / "bin"
    if bin_dir.is_dir():
        bins.update(p.stem for p in bin_dir.glob("*.rs"))

    if (crate / "src" / "main.rs").exists() and package_name:
        bins.add(package_name)

    return bins


def ignored_bins(dockerfile: str) -> set[str]:
    return set(re.findall(r"#\s*dockerfile-bins:\s*ignore\s+(\S+)", dockerfile))


def main() -> int:
    failures: list[str] = []
    checked = 0

    for manifest in sorted(ROOT.glob("*/Cargo.toml")):
        crate = manifest.parent
        dockerfile_path = crate / "Dockerfile"
        if not dockerfile_path.exists():
            continue

        bins = declared_bins(crate)
        if not bins:
            continue

        dockerfile = dockerfile_path.read_text()
        ignored = ignored_bins(dockerfile)

        # A COPY of the release directory itself ships everything in it.
        if re.search(r"COPY[^\n]*/target/release/?\s", dockerfile):
            checked += 1
            continue

        missing = sorted(
            b for b in bins - ignored if not re.search(rf"release/{re.escape(b)}\b", dockerfile)
        )
        checked += 1
        if missing:
            failures.append(
                f"{crate.name}: builds {sorted(bins)} but the Dockerfile never copies "
                f"{missing} into the runtime image"
            )

    if failures:
        print("Binaries that would deploy cleanly and then fail to start:\n", file=sys.stderr)
        for f in failures:
            print(f"  - {f}", file=sys.stderr)
        print(
            "\nAdd a COPY line for each, or opt out with "
            "'# dockerfile-bins: ignore <name>' in the Dockerfile.",
            file=sys.stderr,
        )
        return 1

    print(f"ok — every binary in {checked} crate(s) with a Dockerfile is copied into its image")
    return 0


if __name__ == "__main__":
    sys.exit(main())
