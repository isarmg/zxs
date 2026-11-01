#!/usr/bin/env python3
"""Run isolated disk-observation faults with Cargo's exact current engine libtest.

The Linux runner must already permit ptrace and have cc/strace installed. This
check never changes credentials, ptrace policy, or sandbox configuration.
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
RUNNER = "test/engine/unit/engine/observations/run-io-faults.sh"


def test_artifact(output):
    artifacts = []
    finished = []
    for line in output.splitlines():
        item = json.loads(line)
        if item.get("reason") == "compiler-message":
            message = item["message"]
            print(message.get("rendered") or message["message"], file=sys.stderr)
            if message["level"] in {"warning", "error"}:
                raise ValueError("disk observation build reported a Rust warning or error")
        if item.get("reason") == "build-finished":
            finished.append(item["success"])
        if (item.get("reason") == "compiler-artifact"
                and item["target"]["name"] == "storage_engine"
                and item["target"]["kind"] == ["lib"]
                and item["profile"]["test"] and item.get("executable")):
            artifacts.append(item["executable"])
    if finished != [True] or len(artifacts) != 1:
        raise ValueError(f"expected one successful current storage-engine libtest artifact: {artifacts}")
    return artifacts[0]


def main():
    environment = dict(os.environ)
    environment.pop("LD_PRELOAD", None)
    try:
        built = subprocess.run(
            ["cargo", "test", "--locked", "--offline", "-p", "storage-engine", "--lib",
             "--no-run", "--message-format=json"],
            cwd=ROOT, env=environment, check=True, text=True, stdout=subprocess.PIPE,
        )
        binary = test_artifact(built.stdout)
        with tempfile.TemporaryDirectory(prefix="zxs-disk-observation-") as output:
            subprocess.run(["bash", RUNNER, binary, output], cwd=ROOT,
                           env=environment, check=True)
    except subprocess.CalledProcessError as error:
        if error.stdout:
            print(error.stdout, file=sys.stderr)
        raise


if __name__ == "__main__":
    main()
