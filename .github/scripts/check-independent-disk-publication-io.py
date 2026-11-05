#!/usr/bin/env python3
"""Exercise independent-disk fsync replay using Cargo's exact current libtest."""
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]


def test_artifact(output):
    artifacts = []
    finished = []
    for line in output.splitlines():
        item = json.loads(line)
        if item.get("reason") == "compiler-message":
            message = item["message"]
            print(message.get("rendered") or message["message"], file=sys.stderr)
            if message["level"] in {"warning", "error"}:
                raise ValueError("independent disk publication build reported a Rust warning or error")
        if item.get("reason") == "build-finished":
            finished.append(item["success"])
        if (item.get("reason") == "compiler-artifact"
                and item["target"]["name"] == "storage_engine"
                and item["target"]["kind"] == ["lib"] and item["profile"]["test"]
                and item.get("executable")):
            artifacts.append(item["executable"])
    if finished != [True] or len(artifacts) != 1:
        raise ValueError(f"expected one successful current storage-engine libtest artifact, got {artifacts}")
    return artifacts[0]


def main():
    try:
        built = subprocess.run(
            ["cargo", "test", "--locked", "--offline", "-p", "storage-engine", "--lib",
             "--no-run", "--message-format=json"],
            cwd=ROOT, check=True, text=True, stdout=subprocess.PIPE,
        )
    except subprocess.CalledProcessError as error:
        if error.stdout:
            print(error.stdout, file=sys.stderr)
        raise
    subprocess.run(
        ["bash", "test/engine/unit/disk_authority/publication-io.sh", test_artifact(built.stdout)],
        cwd=ROOT, check=True,
    )


if __name__ == "__main__":
    main()
