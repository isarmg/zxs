#!/usr/bin/env python3
"""Register the same ordinary binary on either immutable benchmark checkout."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tomllib

BASELINE = "e2f33d9882cf7263412d9370b6a703e0b216357d"
CANDIDATE = "13b664e9fd41a356ffb7ed982d21e97abb123626"
SUPPORTED = frozenset((BASELINE, CANDIDATE))
BINARY_NAME = "history-scale-driver"
BASELINE_FEATURE = "history-scale-baseline"


def registration(text: str) -> str:
    """Validate a perf-check manifest before adding a no-dependency target."""
    manifest = tomllib.loads(text)
    if manifest["package"]["name"] != "perf-check":
        raise ValueError("expected the existing perf-check package")
    if manifest.get("dev-dependencies"):
        raise ValueError("perf-check dev-dependencies require a new feature audit")
    dependencies = manifest["dependencies"]
    if dependencies["storage-engine"] != {"workspace": True}:
        raise ValueError("storage-engine must retain its unfeatured workspace edge")
    if "features" in manifest:
        raise ValueError("perf-check features require a new adapter audit")
    if any(target["name"] == BINARY_NAME for target in manifest.get("bin", [])):
        raise ValueError("history scale target is already registered")
    return text.rstrip() + (
        '\n\n[[bin]]\nname = "history-scale-driver"\n'
        'path = "../history-scale-bench/driver.rs"\n\n'
        '[features]\nhistory-scale-baseline = []\n'
    )


def prepare(source: Path) -> dict[str, str]:
    if os.environ.get("GITHUB_ACTIONS") != "true":
        raise RuntimeError("prepare is a GitHub Actions validation-only mutation")
    source = source.resolve(strict=True)
    sha = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=source, text=True
    ).strip()
    if sha not in SUPPORTED:
        raise ValueError(f"unsupported source HEAD: {sha}")
    dirty = subprocess.check_output(
        ["git", "status", "--porcelain"], cwd=source, text=True
    )
    if dirty:
        raise ValueError("source checkout must be pristine before registration")
    toolchain = tomllib.loads((source / "rust-toolchain.toml").read_text())
    if toolchain["toolchain"]["channel"] != "1.99.0":
        raise ValueError("source must pin Rust 1.99.0")
    manifest = source / "tools/perf-check/Cargo.toml"
    replacement = registration(manifest.read_text())
    driver = Path(__file__).with_name("driver.rs")
    destination = source / "tools/history-scale-bench/driver.rs"
    destination.parent.mkdir()
    shutil.copyfile(driver, destination)
    manifest.write_text(replacement)
    return {
        "source_sha": sha,
        "driver_sha256": hashlib.sha256(driver.read_bytes()).hexdigest(),
        "manifest_sha256_before": hashlib.sha256(
            subprocess.check_output(
                ["git", "show", "HEAD:tools/perf-check/Cargo.toml"], cwd=source
            )
        ).hexdigest(),
        "manifest_sha256_after": hashlib.sha256(manifest.read_bytes()).hexdigest(),
        "binary": BINARY_NAME,
        "baseline_feature": BASELINE_FEATURE if sha == BASELINE else "",
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    arguments = parser.parse_args()
    print(json.dumps(prepare(arguments.source), sort_keys=True))


if __name__ == "__main__":
    main()
