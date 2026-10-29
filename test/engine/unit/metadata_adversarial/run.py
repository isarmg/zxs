#!/usr/bin/env python3
"""Run compiled Rust regressions individually with Linux resource limits.

Compilation is a separate GitHub Actions step. No production parser or oracle
is reimplemented here. Each exact Rust case must exit successfully; panics,
signals, timeouts, zero discovered cases and missing evidence are failures.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import resource
import subprocess
import sys
import time


MEMORY_BYTES = 1024 * 1024 * 1024
CPU_SECONDS = 30
WALL_SECONDS = 45


def limits():
    resource.setrlimit(resource.RLIMIT_AS, (MEMORY_BYTES, MEMORY_BYTES))
    resource.setrlimit(resource.RLIMIT_CPU, (CPU_SECONDS, CPU_SECONDS))
    resource.setrlimit(resource.RLIMIT_FSIZE, (256 * 1024 * 1024,) * 2)
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--compile-messages", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--only", help="Run exactly this fully qualified Rust test")
    parser.add_argument("--replay-input", type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    binaries = []
    for line in args.compile_messages.read_text().splitlines():
        message = json.loads(line)
        target = message.get("target", {})
        if (message.get("reason") == "compiler-artifact"
                and message.get("profile", {}).get("test")
                and target.get("name") == "storage_engine"
                and "lib" in target.get("kind", [])
                and message.get("executable")):
            binaries.append(message["executable"])
    if len(binaries) != 1:
        raise RuntimeError(f"expected exactly one engine lib test binary, got {binaries}")
    binary = binaries[0]
    listing = subprocess.run([binary, "--list", "--format", "terse"],
                             capture_output=True, text=True, timeout=WALL_SECONDS,
                             check=True, preexec_fn=limits)
    (args.output / "test-list.txt").write_text(listing.stdout)
    names = sorted(line.removesuffix(": test") for line in listing.stdout.splitlines()
                   if "metadata_adversarial" in line and line.endswith(": test"))
    if args.only:
        names = [name for name in names if name == args.only]
    if not names:
        raise RuntimeError("no metadata adversarial tests were wired into the binary")
    results = []
    for ordinal, name in enumerate(names):
        command = [binary, name, "--exact", "--nocapture", "--test-threads=1"]
        started = time.monotonic()
        log_name = f"{ordinal:03d}-{name.replace('::', '-')}.log"
        status = "failed"
        returncode = None
        case_directory = (args.output / f"case-{ordinal:03d}").resolve()
        case_directory.mkdir(parents=True, exist_ok=True)
        environment = {**os.environ, "RUST_BACKTRACE": "1",
                       "ZXS_METADATA_ADVERSARIAL_CASE_DIR": str(case_directory)}
        if args.replay_input:
            environment["ZXS_METADATA_ADVERSARIAL_REPLAY_INPUT"] = str(args.replay_input.resolve())
        with (args.output / log_name).open("w") as log:
            log.write(json.dumps({"command": command}) + "\n")
            log.flush()
            try:
                completed = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT,
                                           timeout=WALL_SECONDS, preexec_fn=limits,
                                           env=environment)
                returncode = completed.returncode
                status = "passed" if returncode == 0 else "failed"
            except subprocess.TimeoutExpired:
                status = "timeout"
                log.write(f"\nwall limit exceeded: {WALL_SECONDS}s\n")
        result = {"test": name, "status": status, "returncode": returncode,
                  "wall_seconds": time.monotonic() - started, "log": log_name}
        results.append(result)
        print(json.dumps(result), flush=True)
    summary = {"platform": platform.platform(), "binary": binary,
               "limits": {"address_space_bytes": MEMORY_BYTES, "cpu_seconds": CPU_SECONDS,
                          "wall_seconds": WALL_SECONDS, "file_bytes": 256 * 1024 * 1024},
               "tests": results, "passed": sum(r["status"] == "passed" for r in results),
               "failed": sum(r["status"] != "passed" for r in results)}
    if args.replay_input:
        summary["replay_input_sha256"] = hashlib.sha256(args.replay_input.read_bytes()).hexdigest()
    (args.output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    return 1 if summary["failed"] else 0


if __name__ == "__main__":
    sys.exit(main())
