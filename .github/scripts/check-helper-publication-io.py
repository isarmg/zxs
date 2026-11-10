#!/usr/bin/env python3
"""Run helper's isolated publication/CLI faults on exact current libc artifacts."""
import json
from pathlib import Path
import runpy
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]


def test_artifacts(output):
    artifacts = {True: [], False: []}
    finished = []
    for line in output.splitlines():
        item = json.loads(line)
        if item.get("reason") == "compiler-message":
            message = item["message"]
            print(message.get("rendered") or message["message"], file=sys.stderr)
            if message["level"] in {"warning", "error"}:
                raise ValueError("helper publication build reported a Rust warning or error")
        if item.get("reason") == "build-finished":
            finished.append(item["success"])
        if (item.get("reason") == "compiler-artifact"
                and item["target"]["name"] == "zxs-helper"
                and item["target"]["kind"] == ["bin"] and item.get("executable")):
            artifacts[item["profile"]["test"]].append(item["executable"])
    if finished != [True] or any(len(values) != 1 for values in artifacts.values()):
        raise ValueError(f"expected one successful helper bin-test and CLI artifact, got {artifacts}")
    return artifacts[True][0], artifacts[False][0]


def main():
    environment = runpy.run_path(str(ROOT / ".github/scripts/backend_environment.py"))["matrix_environment"]()
    try:
        built = subprocess.run(
            ["cargo", "test", "--locked", "--offline", "-p", "storage-helper",
             "--features", "rustix/use-libc", "--no-run", "--message-format=json"],
            cwd=ROOT, env=environment, check=True, text=True, stdout=subprocess.PIPE,
        )
    except subprocess.CalledProcessError as error:
        print(error.stdout, file=sys.stderr)
        raise
    subprocess.run(
        [sys.executable, str(ROOT / "test/server/service_namespace.py"), "--command",
         "/usr/bin/bash", str(ROOT / "test/helper/unit/private_io/publication-io.sh"),
         *test_artifacts(built.stdout)],
        cwd=ROOT, env=environment, check=True,
    )


if __name__ == "__main__":
    main()
