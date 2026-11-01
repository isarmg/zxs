#!/usr/bin/env python3
"""Exercise actual ID consumers against isolated libc entropy failure."""
import json
from pathlib import Path
import runpy
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
TESTS = {
    "storage_contract": "tests::entropy_fault_returns_typed_failure_without_panic_or_identity",
    "storage_server": "tests::http_boundary_tests::entropy_fault_error_response_does_not_request_entropy",
    "storage_engine": "tests::entropy_failure_tests::entropy_fault_preserves_unpublished_state_and_workspace",
}


def test_artifacts(output):
    artifacts = {name: [] for name in TESTS}
    finished = []
    for line in output.splitlines():
        item = json.loads(line)
        if item.get("reason") == "compiler-message":
            message = item["message"]
            if message["level"] in {"warning", "error"}:
                raise ValueError(message.get("rendered") or message["message"])
        if item.get("reason") == "build-finished":
            finished.append(item["success"])
        if item.get("reason") == "compiler-artifact":
            name = item["target"]["name"]
            if (name in artifacts and "lib" in item["target"]["kind"]
                    and item["profile"]["test"] and item.get("executable")):
                artifacts[name].append(item["executable"])
    if finished != [True] or any(len(paths) != 1 for paths in artifacts.values()):
        raise ValueError("expected one successful current libtest artifact per entropy consumer")
    return {name: paths[0] for name, paths in artifacts.items()}


def main():
    environment = runpy.run_path(str(ROOT / ".github/scripts/backend_environment.py"))["matrix_environment"]()
    command = ["cargo", "test", "--locked", "--offline", "--lib", "--no-run", "--message-format=json"]
    for package in ("storage-contract", "storage-engine", "storage-server"):
        command += ["-p", package]
    try:
        built = subprocess.run(command, cwd=ROOT, env=environment, check=True,
                               text=True, stdout=subprocess.PIPE)
    except subprocess.CalledProcessError as error:
        print(error.stdout, file=sys.stderr)
        raise
    for name, binary in test_artifacts(built.stdout).items():
        print(f"Entropy fault artifact {name}: {binary}", flush=True)
        subprocess.run(["bash", "test/contract/entropy/entropy-io.sh", binary, TESTS[name]],
                       cwd=ROOT, env=environment, check=True)


if __name__ == "__main__":
    main()
