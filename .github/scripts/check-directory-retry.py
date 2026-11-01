#!/usr/bin/env python3
"""Run directory retry barriers against both exact current Cargo libtests."""
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
TARGETS = {"platform_io": "platform", "storage_engine": "engine"}


def test_artifacts(output):
    artifacts = {target: [] for target in TARGETS}
    finished = []
    for line in output.splitlines():
        item = json.loads(line)
        if item.get("reason") == "compiler-message":
            message = item["message"]
            print(message.get("rendered") or message["message"], file=sys.stderr)
            if message["level"] in {"warning", "error"}:
                raise ValueError("directory retry build reported a Rust warning or error")
        if item.get("reason") == "build-finished":
            finished.append(item["success"])
        if (item.get("reason") == "compiler-artifact"
                and item["target"]["name"] in TARGETS
                and item["target"]["kind"] == ["lib"] and item["profile"]["test"]
                and item.get("executable")):
            artifacts[item["target"]["name"]].append(item["executable"])
    if finished != [True] or any(len(paths) != 1 for paths in artifacts.values()):
        raise ValueError(f"expected one successful libtest per directory retry target, got {artifacts}")
    return {TARGETS[target]: paths[0] for target, paths in artifacts.items()}


def main():
    try:
        built = subprocess.run(
            ["cargo", "test", "--locked", "--offline", "-p", "platform-io",
             "-p", "storage-engine", "--lib", "--no-run", "--message-format=json"],
            cwd=ROOT, check=True, text=True, stdout=subprocess.PIPE,
        )
    except subprocess.CalledProcessError as error:
        print(error.stdout, file=sys.stderr)
        raise
    for component, binary in test_artifacts(built.stdout).items():
        subprocess.run(
            ["bash", "test/platform/unit/fs_safety/directory-retry.sh", binary, component],
            cwd=ROOT, check=True,
        )


if __name__ == "__main__":
    main()
