#!/usr/bin/env python3
"""Reuse package unit-test binaries for bounded membership and disk-IO groups."""
import argparse
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
IO_ENGINE_GROUPS = (
    ("disk-admission", "disk::admission::tests::", ()),
    ("io-authority", "engine::config::io_authority::tests::", ()),
    ("guarded-engine-reads", "engine::reads::tests::", ()),
    ("verified-payloads", "engine::verified_payloads::tests::", ()),
    ("workspace-range", "business::file::pending_regressions::", ()),
)
IO_SERVER_GROUPS = (
    ("http-transfer", "transfer::tests::", ()),
    ("smb-stream-read", "smb_rpc::tests::stream_read_tests::", ()),
    ("smb-bounded-lifecycle", "smb_rpc::tests::bounded_lifecycle_tests::", ()),
    ("smb-close", "smb_rpc::tests::close_tests::", ()),
)


def build_command(package):
    command = ["cargo", "test", "-p", package, "--lib"]
    if package == "storage-engine":
        command += ["--features", "fault-injection"]
    # Server unit tests enable the engine's hooks through their dev-dependency;
    # storage-server itself has no fault-injection feature.
    return command + ["--locked", "--offline", "--no-run", "--message-format=json"]


def build(package):
    spec = importlib.util.spec_from_file_location(
        "rust_diagnostics", ROOT / ".github/scripts/check-rust-diagnostics.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    diagnostics = module.CargoDiagnostics(sys.stderr)
    command = build_command(package)
    executables = set()
    with (EVIDENCE / f"{package}-build.jsonl").open("w") as log:
        with subprocess.Popen(command, cwd=ROOT, stdout=subprocess.PIPE,
                              text=True, encoding="utf-8") as process:
            for number, line in enumerate(process.stdout, 1):
                log.write(line)
                diagnostics.consume(line, number)
                if diagnostics.invalid:
                    continue
                event = json.loads(line)
                if (event.get("reason") == "compiler-artifact"
                        and event["target"]["name"] == package.replace("-", "_")
                        and event["target"]["kind"] == ["lib"]
                        and event["profile"]["test"] and event["executable"]):
                    executables.add(event["executable"])
            status = diagnostics.result(process.wait())
    if status:
        raise RuntimeError(f"{package} unit-test build failed with status {status}")
    if len(executables) != 1:
        raise RuntimeError(f"expected one {package} unit-test artifact, got {executables}")
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
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--disk-io", action="store_true",
                        help="also validate guarded engine, HTTP and SMB read lifetimes")
    options = parser.parse_args()
    EVIDENCE.mkdir(parents=True, exist_ok=True)
    summary = {"commit": subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
        "disk_io": options.disk_io, "binaries": {}, "groups": []}
    try:
        packages = [("storage-engine", GROUPS + (IO_ENGINE_GROUPS if options.disk_io else ()))]
        if options.disk_io:
            packages.append(("storage-server", IO_SERVER_GROUPS))
        for package, groups in packages:
            binary = build(package)
            with binary.open("rb") as source:
                summary["binaries"][package] = hashlib.file_digest(source, "sha256").hexdigest()
            for label, test_filter, extra in groups:
                result = run_group(binary, label, test_filter, extra)
                summary["groups"].append({"package": package, **result})
        return int(any(group["exit_code"] != 0 for group in summary["groups"]))
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        summary["error"] = str(error)
        print(error, file=sys.stderr)
        return 1
    finally:
        (EVIDENCE / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")


if __name__ == "__main__":
    sys.exit(main())
