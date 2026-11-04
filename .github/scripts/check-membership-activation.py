#!/usr/bin/env python3
"""Build only engine unit tests, then reuse that binary for bounded CI groups."""
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
EVIDENCE = ROOT / "target/membership-validation"
GROUPS = (
    ("activation", "membership_activation_", ()),
    ("membership", "membership_", ("--skip", "membership_activation_")),
    ("recovery", "engine::recovery::", ()),
    ("degraded-recovery", "degraded_recovery::tests::", ()),
    ("proof-decoding", "journal::proof_decoding_tests::", ()),
    ("journal-scanner", "journal::scanner_tests::", ()),
    ("workspace-storage", "workspace_storage::tests::", ()),
    ("history-package", "history_package::tests::", ()),
    ("scrub", "engine::integrity_scrub::tests::", ()),
    ("disk-identity", "disk::identity_tests::", ()),
    ("read-provenance", "version_health_fallback_observes_shared_membership", ()),
)


def build():
    spec = importlib.util.spec_from_file_location(
        "rust_diagnostics", ROOT / ".github/scripts/check-rust-diagnostics.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    diagnostics = module.CargoDiagnostics(sys.stderr)
    command = ["cargo", "test", "-p", "storage-engine", "--lib", "--features",
               "fault-injection", "--locked", "--offline", "--no-run",
               "--message-format=json"]
    executables = set()
    with (EVIDENCE / "build.jsonl").open("w") as log:
        with subprocess.Popen(command, cwd=ROOT, stdout=subprocess.PIPE,
                              text=True, encoding="utf-8") as process:
            for number, line in enumerate(process.stdout, 1):
                log.write(line)
                diagnostics.consume(line, number)
                if diagnostics.invalid:
                    continue
                event = json.loads(line)
                if (event.get("reason") == "compiler-artifact"
                        and event["target"]["name"] == "storage_engine"
                        and event["target"]["kind"] == ["lib"]
                        and event["profile"]["test"] and event["executable"]):
                    executables.add(event["executable"])
            status = diagnostics.result(process.wait())
    if status:
        raise RuntimeError(f"engine unit-test build failed with status {status}")
    if len(executables) != 1:
        raise RuntimeError(f"expected one engine unit-test artifact, got {executables}")
    return Path(executables.pop())


def run_group(binary, label, test_filter, extra):
    command = [str(binary), test_filter, *extra, "--test-threads=1", "--nocapture"]
    # A renamed/removed test group must fail rather than silently run zero tests.
    inventory = subprocess.run([str(binary), test_filter, *extra, "--list"],
                               cwd=ROOT, text=True, capture_output=True, check=True)
    tests = [line for line in inventory.stdout.splitlines() if line.endswith(": test")]
    if not tests:
        raise RuntimeError(f"empty test group: {label} ({test_filter})")
    with (EVIDENCE / f"{label}.log").open("w") as log:
        log.write("Command: " + " ".join(command) + "\n")
        with subprocess.Popen(command, cwd=ROOT, stdout=subprocess.PIPE,
                              stderr=subprocess.STDOUT, text=True,
                              encoding="utf-8") as process:
            for line in process.stdout:
                log.write(line)
                print(line, end="", flush=True)
            status = process.wait()
    return {"group": label, "filter": test_filter, "listed_tests": len(tests),
            "exit_code": status}


def main():
    EVIDENCE.mkdir(parents=True, exist_ok=True)
    summary = {"commit": subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(), "groups": []}
    try:
        binary = build()
        with binary.open("rb") as source:
            summary["unit_binary_sha256"] = hashlib.file_digest(source, "sha256").hexdigest()
        for label, test_filter, extra in GROUPS:
            summary["groups"].append(run_group(binary, label, test_filter, extra))
        return int(any(group["exit_code"] != 0 for group in summary["groups"]))
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        summary["error"] = str(error)
        print(error, file=sys.stderr)
        return 1
    finally:
        (EVIDENCE / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")


if __name__ == "__main__":
    sys.exit(main())
