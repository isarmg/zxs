#!/usr/bin/env python3
"""Run isolated atomic barriers using Cargo's exact current server libtest."""
import json
import runpy
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
                raise ValueError("auth fault I/O build reported a Rust warning or error")
        if item.get("reason") == "build-finished":
            finished.append(item["success"])
        if (item.get("reason") == "compiler-artifact"
                and item["target"]["name"] == "storage_server"
                and "lib" in item["target"]["kind"] and item["profile"]["test"]
                and item.get("executable")):
            artifacts.append(item["executable"])
    if finished != [True] or len(artifacts) != 1:
        raise ValueError(f"expected one successful storage-server libtest artifact, got {artifacts}")
    return artifacts[0]


def main():
    environment = runpy.run_path(str(ROOT / ".github/scripts/backend_environment.py"))["matrix_environment"]()
    platform_artifact = runpy.run_path(str(ROOT / ".github/scripts/check-platform-atomic-io.py"))["test_artifact"]
    for suite, features in (("barriers", []), ("rename", ["--features", "rustix/use-libc"])):
        # Normal production backend still runs its full barrier matrix. Only the
        # explicit libc build admits LD_PRELOAD around actual rename syscalls.
        try:
            built = subprocess.run(
                ["cargo", "test", "--locked", "--offline", "-p", "storage-server",
                 "-p", "platform-io", "--lib", "--no-run", "--message-format=json", *features],
                cwd=ROOT, env=environment, check=True, text=True, stdout=subprocess.PIPE,
            )
        except subprocess.CalledProcessError as error:
            print(error.stdout, file=sys.stderr)
            raise
        subprocess.run(
            ["bash", "test/server/unit/auth/auth-io.sh", test_artifact(built.stdout), suite],
            cwd=ROOT, env=environment, check=True,
        )
        if suite == "rename":
            subprocess.run(
                ["bash", "test/platform/unit/fs_safety/atomic-io.sh", platform_artifact(built.stdout), suite],
                cwd=ROOT, env=environment, check=True,
            )


if __name__ == "__main__":
    main()
