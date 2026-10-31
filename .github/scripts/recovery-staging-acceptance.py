#!/usr/bin/env python3
"""Run bounded recovery/staging acceptance against an immutable source checkout."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time


BASE = "d3eefd289e53a2803703870fe4250d08cadff9b5"
PREFIXES = (
    "engine::recovery_checkpoint::tests::",
    "engine::recovery::tests::",
    "journal::writer_lifecycle_tests::",
    "tests::historical_workspace_lifecycle::",
    "tests::retained_workspace_budget::",
    "tests::runtime_workspace_limits::",
    "tests::cold_workspace_records::",
    "tests::published_workspace_retirement::",
)
INTEGRATIONS = (
    "historical_workspace_lifecycle",
    "workspace_commit_replay",
    "workspace_upload_retries",
)
IO_FAILURE = "workspace_evidence_io_failure_is_not_absence_or_corrupt_mirror"
PERMISSION_FAILURE = "unreadable_workspace_request_is_not_a_missing_mirror"
NEW_TESTS = (
    "checkpoint_hit_preserves_cold_history_full_generations_and_exact_staging_budget",
    "corrupt_checkpoint_fallback_preserves_cold_history_full_generations_and_exact_staging_budget",
    "member_checkpoint_mismatch_preserves_cold_history_full_generations_and_exact_staging_budget",
    "torn_tail_recovery_preserves_cold_history_full_generations_and_exact_staging_budget",
)


def git(source, *args):
    return subprocess.check_output(["git", "-C", str(source), *args]).decode().strip()


def tracked_hashes(source):
    names = subprocess.check_output(
        ["git", "-C", str(source), "ls-files", "-z"]
    ).split(b"\0")
    result = {}
    for raw in names:
        if not raw:
            continue
        name = os.fsdecode(raw)
        path = source / name
        data = os.fsencode(os.readlink(path)) if path.is_symlink() else path.read_bytes()
        result[name] = hashlib.sha256(data).hexdigest()
    return result


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def run(source, evidence, name, command, records, env_changes=None):
    print(f"BEGIN {name}: {command}", flush=True)
    start = time.monotonic()
    log = evidence / f"{name}.log"
    with log.open("w") as output:
        output.write(json.dumps(command) + "\n")
        output.flush()
        completed = subprocess.run(
            command, cwd=source, stdout=output, stderr=subprocess.STDOUT,
            env=None if env_changes is None else dict(os.environ, **env_changes),
        )
    records.append(
        {"name": name, "command": command, "returncode": completed.returncode,
         "seconds": time.monotonic() - start, "log": log.name,
         "env_changes": env_changes or {}}
    )
    print(f"END {name}: returncode={completed.returncode}", flush=True)
    if completed.returncode:
        print(log.read_text()[-16000:], flush=True)
        raise RuntimeError(f"{name} failed with returncode {completed.returncode}")
    return log.read_text()


def inventory(text):
    return sorted(re.findall(r"^(.+): test$", text, re.MULTILINE))


def checked_tests(text, expected):
    actual = sorted(re.findall(r"^test (\S+) \.\.\. ok$", text, re.MULTILINE))
    summaries = re.findall(
        r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;", text
    )
    if actual != expected or summaries != [(str(len(expected)), "0", "0")]:
        raise RuntimeError(
            f"execution differs from inventory: expected={expected!r}, "
            f"actual={actual!r}, summaries={summaries!r}"
        )
    return len(actual)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--source-tree", required=True)
    parser.add_argument("--carrier-sha", required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    args = parser.parse_args()
    source = args.source.resolve()
    evidence = args.evidence.resolve()
    evidence.mkdir(parents=True, exist_ok=True)
    records = []
    report = {
        "source_sha": args.source_sha, "source_tree": args.source_tree,
        "carrier_sha": args.carrier_sha, "base": BASE, "uid": os.getuid(),
        "commands": records, "groups": [], "status": "failed",
    }
    before = None
    try:
        if os.getuid() == 0:
            raise RuntimeError("acceptance must execute as the normal unprivileged runner user")
        if git(source, "rev-parse", "HEAD") != args.source_sha:
            raise RuntimeError("source commit pin mismatch")
        if git(source, "rev-parse", "HEAD^{tree}") != args.source_tree:
            raise RuntimeError("source tree pin mismatch")
        if git(source, "status", "--porcelain", "--untracked-files=all"):
            raise RuntimeError("source checkout starts dirty")
        changed = git(source, "diff", "--name-only", BASE, "HEAD").splitlines()
        if not changed or any(
            not name.startswith("test/engine/unit/engine/recovery_checkpoint/")
            for name in changed
        ):
            raise RuntimeError(f"candidate exceeds reviewed test-only scope: {changed!r}")
        report["changed_paths"] = changed
        before = tracked_hashes(source)
        write_json(evidence / "tracked-before.json", before)
        run(source, evidence, "diff-check", ["git", "diff", "--check", BASE, "HEAD"], records)
        toolchain = run(source, evidence, "toolchain", ["rustc", "--version"], records)
        if not re.search(r"^rustc 1\.99\.0 ", toolchain, re.MULTILINE):
            raise RuntimeError("repository Rust 1.99.0 toolchain required")
        run(source, evidence, "rustfmt-version", ["rustfmt", "--version"], records)
        run(source, evidence, "rustfmt-check", [
            "rustfmt", "--check", "--edition", "2024", "--config", "skip_children=true",
            *[name for name in changed if name.endswith(".rs")],
        ], records)
        for name, script in (
            ("architecture", "check-architecture.py"),
            ("native-features", "check-native-consumer-features.py"),
            ("history-object-io", "check-history-object-io.py"),
            ("platform-atomic-io", "check-platform-atomic-io.py"),
        ):
            run(source, evidence, name, ["python3", f".github/scripts/{script}"], records)
        run(source, evidence, "source-governance", [
            "python3", "tools/rust-sources/manage.py", "verify",
        ], records)
        cargo = ["cargo", "test", "--locked", "--offline", "-p", "storage-engine"]
        listed = run(source, evidence, "unit-inventory", cargo + ["--lib", "--", "--list"], records)
        names = inventory(listed)
        selected = sorted(name for name in names if name.startswith(PREFIXES))
        counts = {prefix: sum(name.startswith(prefix) for name in names) for prefix in PREFIXES}
        if not all(counts.values()):
            raise RuntimeError(f"missing unit test inventory: {counts!r}")
        new = [name for name in selected if "::lifecycle_tests::" in name]
        expected_new = sorted(
            "engine::recovery_checkpoint::tests::lifecycle_tests::" + name
            for name in NEW_TESTS
        )
        if new != expected_new:
            raise RuntimeError(f"reviewed cross-scenario inventory differs: {new!r}")
        write_json(evidence / "unit-inventory.json", {"counts": counts, "selected": selected, "new": new})
        output = run(source, evidence, "unit-acceptance", cargo + [
            "--lib", "--", *PREFIXES, "--test-threads=1",
        ], records)
        report["groups"].append({"name": "unit", "tests": checked_tests(output, selected), "counts": counts})
        for target in INTEGRATIONS:
            command = cargo + ["--test", target]
            listed = run(source, evidence, f"{target}-inventory", command + ["--", "--list"], records)
            selected = inventory(listed)
            if not selected:
                raise RuntimeError(f"zero-test integration inventory: {target}")
            write_json(evidence / f"{target}-inventory.json", selected)
            filters = []
            if target == "workspace_commit_replay":
                for ignored in (IO_FAILURE, PERMISSION_FAILURE):
                    if ignored not in selected:
                        raise RuntimeError(f"missing explicit ignored-test inventory: {ignored}")
                    selected.remove(ignored)
                    filters += ["--skip", ignored]
            output = run(source, evidence, f"{target}-acceptance", command + ["--", "--test-threads=1", *filters], records)
            report["groups"].append({"name": target, "tests": checked_tests(output, selected)})
        command = cargo + ["--test", "workspace_commit_replay", "--", "--ignored", "--exact"]
        output = run(source, evidence, "real-permission-failure", command + [PERMISSION_FAILURE, "--test-threads=1"], records)
        report["groups"].append({"name": "real-permission-failure", "tests": checked_tests(output, [PERMISSION_FAILURE])})
        interposer = evidence / "workspace_read_error.so"
        run(source, evidence, "interposer-compiler", ["cc", "--version"], records)
        run(source, evidence, "compile-read-interposer", [
            "cc", "-shared", "-fPIC", "-Wall", "-Wextra", "-Werror",
            "test/engine/integration/workspace_read_error.c", "-ldl", "-o", str(interposer),
        ], records)
        for errno in (5, 13):
            name = f"workspace-read-errno-{errno}"
            output = run(source, evidence, name, command + [IO_FAILURE, "--test-threads=1"], records,
                         {"LD_PRELOAD": str(interposer), "ZXS_TEST_WORKSPACE_ERRNO": str(errno)})
            report["groups"].append({"name": name, "tests": checked_tests(output, [IO_FAILURE]), "errno": errno})
        report["total_executions"] = sum(group["tests"] for group in report["groups"])
        report["unique_tests"] = report["total_executions"] - 1
        report["status"] = "passed"
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
        print(report["error"], flush=True)
    finally:
        try:
            after = tracked_hashes(source)
            write_json(evidence / "tracked-after.json", after)
            report["tracked_source_unchanged"] = before is not None and before == after
            report["git_status_after"] = git(source, "status", "--porcelain", "--untracked-files=all")
            if not report["tracked_source_unchanged"] or report["git_status_after"]:
                report["status"] = "failed"
                report["source_error"] = "source changed during validation"
        except Exception as error:
            report["status"] = "failed"
            report["source_error"] = f"{type(error).__name__}: {error}"
        write_json(evidence / "summary.json", report)
        print(json.dumps(report, indent=2), flush=True)
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
