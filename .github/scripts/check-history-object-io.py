#!/usr/bin/env python3
"""Run isolated existing-object durability tests from Cargo's exact libtest artifact."""
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]


def main():
    try:
        built = subprocess.run(
            ["cargo", "test", "--locked", "--offline", "-p", "storage-engine", "--lib",
             "--no-run", "--message-format=json"],
            cwd=ROOT, check=True, text=True, stdout=subprocess.PIPE,
        )
    except subprocess.CalledProcessError as error:
        print(error.stdout, file=sys.stderr)
        raise
    artifacts = []
    for line in built.stdout.splitlines():
        item = json.loads(line)
        if item.get("reason") == "compiler-message":
            print(item["message"].get("rendered") or item["message"]["message"], file=sys.stderr)
        if (item.get("reason") == "compiler-artifact"
                and item["target"]["name"] == "storage_engine"
                and "lib" in item["target"]["kind"] and item["profile"]["test"]
                and item.get("executable")):
            artifacts.append(item["executable"])
    if len(artifacts) != 1:
        raise ValueError(f"expected one storage-engine libtest artifact, got {artifacts}")
    subprocess.run(
        ["bash", "test/engine/unit/history_package/immutable_io/run.sh", artifacts[0]],
        cwd=ROOT, check=True,
    )


if __name__ == "__main__":
    main()
