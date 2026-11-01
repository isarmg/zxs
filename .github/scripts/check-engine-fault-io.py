#!/usr/bin/env python3
"""Run error-source regressions with current artifacts and isolated libc faults."""
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
TARGETS = {"storage_engine": ["lib"], "workspace_commit_replay": ["test"]}
READ_TEST = "workspace_evidence_io_failure_is_not_absence_or_corrupt_mirror"
PERMISSION_TEST = "unreadable_workspace_request_is_not_a_missing_mirror"
SYNC_TEST = "container_storage::tests::existing_body_sync_io_failure_retains_its_cause"


def test_artifacts(output):
    artifacts = {target: [] for target in TARGETS}
    finished = []
    for line in output.splitlines():
        item = json.loads(line)
        if item.get("reason") == "compiler-message":
            message = item["message"]
            print(message.get("rendered") or message["message"], file=sys.stderr)
            if message["level"] in {"warning", "error"}:
                raise ValueError("engine fault build reported a Rust warning or error")
        if item.get("reason") == "build-finished":
            finished.append(item["success"])
        if item.get("reason") == "compiler-artifact":
            target = item["target"]
            if (target["name"] in TARGETS and target["kind"] == TARGETS[target["name"]]
                    and item["profile"]["test"] and item.get("executable")):
                artifacts[target["name"]].append(item["executable"])
    if finished != [True] or any(len(paths) != 1 for paths in artifacts.values()):
        raise ValueError(f"expected one successful current artifact per engine fault target: {artifacts}")
    return {target: paths[0] for target, paths in artifacts.items()}


def run_exact(binary, name, marker, environment, injections=()):
    listed = subprocess.run(
        [binary, name, "--ignored", "--exact", "--list"], cwd=ROOT,
        env=environment, check=True, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
    )
    if listed.stdout.splitlines().count(f"{name}: test") != 1:
        raise ValueError(f"missing exact ignored engine fault test: {name}")
    result = subprocess.run(
        [binary, name, "--ignored", "--exact", "--nocapture", "--test-threads=1"],
        cwd=ROOT, env=environment, check=True, text=True,
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
    )
    print(result.stdout, end="")
    if (result.stdout.splitlines().count(marker) != 1
            or len(re.findall(r"^test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; \d+ filtered out;.*$",
                              result.stdout, re.MULTILINE)) != 1
            or any(not re.search(pattern, result.stdout, re.MULTILINE) for pattern in injections)):
        raise ValueError(f"missing injection, completion or single-test pass: {name}")


def main():
    # CI's ordinary non-root runner executes the real DAC test, without sudo,
    # persistent accounts or a permission-based skip. The Rust probe also rejects
    # capability-based bypass by requiring an actual kernel PermissionDenied.
    if os.geteuid() == 0:
        raise ValueError("engine fault gate requires a non-root runner for real EACCES")
    try:
        built = subprocess.run(
            ["cargo", "test", "--locked", "--offline", "-p", "storage-engine", "--lib",
             "--test", "workspace_commit_replay", "--no-run", "--message-format=json"],
            cwd=ROOT, check=True, text=True, stdout=subprocess.PIPE,
        )
        artifacts = test_artifacts(built.stdout)
        environment = dict(os.environ)
        for key in ("LD_PRELOAD", "ZXS_TEST_WORKSPACE_ERRNO"):
            environment.pop(key, None)
        with tempfile.TemporaryDirectory(prefix="zxs-engine-fault-") as temporary:
            libraries = {}
            for name, source in (
                ("read", "test/engine/integration/workspace_read_error.c"),
                ("fsync", "test/engine/unit/container_storage/fsync_eio.c"),
            ):
                libraries[name] = str(Path(temporary) / f"{name}.so")
                subprocess.run(["cc", "-Wall", "-Wextra", "-Werror", "-shared", "-fPIC",
                                source, "-ldl", "-o", libraries[name]], cwd=ROOT, check=True)
            for errno in (5, 13):
                run_exact(artifacts["workspace_commit_replay"], READ_TEST,
                          f"ZXS_WORKSPACE_READ_COMPLETE errno={errno}",
                          dict(environment, LD_PRELOAD=libraries["read"], ZXS_TEST_WORKSPACE_ERRNO=str(errno)),
                          tuple(rf"^ZXS_TEST_WORKSPACE_READ errno={errno} .*\/{file}\.read-error$"
                                for file in ("commit-request", "workspace")))
            run_exact(artifacts["storage_engine"], SYNC_TEST, "ZXS_EXISTING_BODY_SYNC_COMPLETE",
                      dict(environment, LD_PRELOAD=libraries["fsync"]), (r"^ZXS_TEST_FSYNC_EIO /.+$",))
            run_exact(artifacts["workspace_commit_replay"], PERMISSION_TEST,
                      "ZXS_WORKSPACE_PERMISSION_COMPLETE", environment)
    except subprocess.CalledProcessError as error:
        if error.stdout:
            print(error.stdout, file=sys.stderr)
        raise


if __name__ == "__main__":
    main()
