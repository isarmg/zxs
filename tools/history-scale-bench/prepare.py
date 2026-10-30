#!/usr/bin/env python3
"""Register the same ordinary binary on either immutable benchmark checkout."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import tomllib
from controller import BASELINE, reviewed_candidate as candidate_revision

BINARY_NAME = "history-scale-driver"
BASELINE_FEATURE = "history-scale-baseline"
CURRENT_FEATURE = "history-scale-current"
DRIVER_PATH = "tools/history-scale-bench/driver.rs"


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
        '[features]\nhistory-scale-baseline = []\nhistory-scale-current = []\n'
    )


def place_driver(source: Path, driver: Path) -> str:
    destination = source / DRIVER_PATH
    tracked = subprocess.check_output(["git", "ls-files", "--", DRIVER_PATH], cwd=source, text=True).strip()
    if tracked:
        if tracked != DRIVER_PATH or not stat.S_ISREG(destination.lstat().st_mode):
            raise ValueError("tracked driver must be the expected regular source file")
        if destination.read_bytes() != driver.read_bytes():
            raise ValueError("tracked driver differs from the measurement carrier; no overwrite")
        return "tracked"
    destination.parent.mkdir(parents=True, exist_ok=True)
    with destination.open("xb") as output:
        output.write(driver.read_bytes())
    return "copied"


def prepare(source: Path, candidate_sha: str) -> dict:
    if os.environ.get("GITHUB_ACTIONS") != "true":
        raise RuntimeError("prepare is a GitHub Actions validation-only mutation")
    source = source.resolve(strict=True)
    candidate_sha = candidate_revision(candidate_sha)
    sha = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=source, text=True
    ).strip()
    if sha not in {BASELINE, candidate_sha}:
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
    placement = place_driver(source, driver)
    manifest.write_text(replacement)
    return {
        "schema_version": 2,
        "driver_registration": placement,
        "source_sha": sha,
        "reviewed_candidate_sha": candidate_sha,
        "adapter": "baseline" if sha == BASELINE else "current",
        "build_features": [BASELINE_FEATURE if sha == BASELINE else CURRENT_FEATURE],
        "lockfile_sha256": hashlib.sha256((source / "Cargo.lock").read_bytes()).hexdigest(),
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
    parser.add_argument("--candidate-sha", required=True)
    arguments = parser.parse_args()
    print(json.dumps(prepare(arguments.source, arguments.candidate_sha), sort_keys=True))


if __name__ == "__main__":
    main()
