#!/usr/bin/env python3
"""Record source overlays, toolchain identities and exact measurement binaries."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

FIXTURES = (
    "test/engine/unit/engine/payload_measurement.rs",
)


def command(arguments, cwd):
    return subprocess.run(arguments, cwd=cwd, check=True, capture_output=True).stdout


def sha256(path):
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def source_overlay(source):
    patch = command(["git", "diff", "--binary", "HEAD"], source)
    untracked = command(["git", "ls-files", "--others", "--exclude-standard", "-z"], source)
    for name in untracked.split(b"\0"):
        if name:
            result = subprocess.run(
                ["git", "diff", "--no-index", "--binary", "--", "/dev/null", name.decode()],
                cwd=source, capture_output=True,
            )
            if result.returncode not in (0, 1):
                result.check_returncode()
            patch += result.stdout
    return patch


def record(baseline, candidate, output):
    patch = output / "baseline-overlay.patch"
    patch.write_bytes(source_overlay(baseline))
    sources = {}
    for label, source in (("baseline", baseline), ("candidate", candidate)):
        binary = Path((output / f"{label}-binary.txt").read_text().strip()).resolve()
        sources[label] = {
            "commit": command(["git", "rev-parse", "HEAD"], source).decode().strip(),
            "fixtures_sha256": {name: sha256(source / name) for name in FIXTURES},
            "binary": {"path": str(binary), "sha256": sha256(binary)},
            "toolchain": {
                tool: command([tool, "--version", "--verbose"], source).decode().strip()
                for tool in ("rustc", "cargo")
            },
        }
    provenance = {
        "format_version": 1,
        "sources": sources,
        "baseline_overlay": {"path": patch.name, "sha256": sha256(patch)},
    }
    (output / "provenance.json").write_text(json.dumps(provenance, indent=2) + "\n")
    return provenance


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline_source", type=Path)
    parser.add_argument("candidate_source", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    record(args.baseline_source.resolve(), args.candidate_source.resolve(), args.output.resolve())


if __name__ == "__main__":
    main()
