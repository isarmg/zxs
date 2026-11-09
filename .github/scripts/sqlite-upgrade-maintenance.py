#!/usr/bin/env python3
"""GitHub-only, fixed SQLite source maintenance. Emit a patch; never publish it."""
import importlib.util
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time

from sqlite_upgrade.archive import IDENTITY, URL, SHA256, download, sha256
from sqlite_upgrade.upgrade import DIRECTORY, TEST_ROOT, PROVENANCE, upgrade, json_bytes

ROOT = Path(__file__).resolve().parents[2]
LIMIT_SECONDS = 11 * 60  # Inside the workflow's 15-minute whole-job limit.
START = time.monotonic()


def run(command):
    # All callers supply fixed argv, never an input/environment shell command.
    remaining = max(1, int(LIMIT_SECONDS - (time.monotonic() - START)))
    result = subprocess.run(command, cwd=ROOT, check=True, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, timeout=remaining)
    return result.stdout


def load_source_tools():
    sys.path.insert(0, str(ROOT / "tools/rust-sources"))
    import manage
    return manage


def require_workflow():
    if len(sys.argv) != 1:
        raise ValueError("this maintenance job accepts no arguments")
    expected = {"GITHUB_ACTIONS": "true", "GITHUB_REPOSITORY": "isarmg/zxs",
                "GITHUB_EVENT_NAME": "workflow_dispatch", "GITHUB_REF": "refs/heads/main"}
    if any(os.environ.get(key) != value for key, value in expected.items()):
        raise ValueError("only the reviewed manual main-branch GitHub job may prepare this patch")
    sha = os.environ.get("GITHUB_SHA", "")
    if not re.fullmatch(r"[0-9a-f]{40}", sha) or run(["git", "rev-parse", "HEAD"]).decode().strip() != sha:
        raise ValueError("checkout does not match the exact dispatched commit")
    if run(["git", "status", "--porcelain=v1", "--untracked-files=all"]):
        raise ValueError("maintenance requires a clean checkout")
    if os.environ.get("RUSTFLAGS") or os.environ.get("CARGO_ENCODED_RUSTFLAGS"):
        raise ValueError("unexpected Rust compiler overrides")
    spec = importlib.util.spec_from_file_location("development_ci", ROOT / ".github/scripts/development-ci.py")
    policy = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(policy)
    policy.check_workflows(ROOT)
    return sha


def emit_patch(destination, mapping_path):
    exact = {
        "Cargo.lock", "src/platform/Cargo.toml",
        "src/foundation/data-storage/sqlite-access/Cargo.toml",
        "compliance/provenance/origins.json", "compliance/rust-sources.json",
        "compliance/NOTICE.md", "docs/THIRD_PARTY_RUST_SOURCES.md",
        "docs/architecture/foundation-consumers.md",
        "compliance/external-tests/sqlite-0.39.1.json", mapping_path,
    }
    prefixes = (DIRECTORY + "/", PROVENANCE + "/",
                "compliance/licenses/upstream/" + IDENTITY + "/",
                "docs/upstream/" + IDENTITY + "/", TEST_ROOT + "/",
                "test/foundation/libsqlite3-sys/")
    untracked = run(["git", "ls-files", "--others", "--exclude-standard", "-z"]).decode().split("\0")
    untracked = [p for p in untracked if p]
    changed = run(["git", "diff", "--name-only", "-z", "HEAD", "--"]).decode().split("\0")
    paths = sorted(set(untracked) | {p for p in changed if p})
    if not paths or any(p not in exact and not p.startswith(prefixes) for p in paths):
        raise ValueError("generated changes escape the exact SQLite source/provenance scope")
    if untracked:
        run(["git", "add", "--intent-to-add", "--", *untracked])
    patch = run(["git", "diff", "--binary", "--full-index", "--no-renames",
                 "--no-ext-diff", "--no-textconv", "HEAD", "--"])
    if not patch or len(patch) > 64 * 1024 * 1024:
        raise ValueError("empty or oversized upgrade patch")
    # Check against this exact resulting checkout, without applying or executing it.
    subprocess.run(["git", "apply", "--check", "--reverse", "--binary", "-"],
                               input=patch, cwd=ROOT, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, check=True, timeout=30)
    temporary = destination.with_suffix(".pending")
    temporary.write_bytes(patch)
    temporary.replace(destination)
    return {"sha256": sha256(patch), "bytes": len(patch), "paths": paths}


def main():
    if os.environ.get("GITHUB_ACTIONS") != "true":
        raise SystemExit("Refusing local execution; use the reviewed GitHub manual workflow.")
    directory = Path(os.environ["RUNNER_TEMP"]) / "zxs-sqlite-0391"
    directory.mkdir(exist_ok=False)
    evidence_dir = directory / "evidence"
    evidence_dir.mkdir()
    evidence = {
        "status": "failed", "stage": "workflow-admission",
        "archive_url": URL, "archive_sha256": SHA256,
        "run_id": os.environ.get("GITHUB_RUN_ID"),
        "run_attempt": os.environ.get("GITHUB_RUN_ATTEMPT"),
        "not_executed": ["compilation", "tests", "security audit", "commit", "push", "deployment"],
        "acceptance": "Source/provenance patch preparation only. Not a CI pass or an installed security fix.",
    }

    def deadline(signum, frame):
        raise TimeoutError("SQLite source preparation exceeded its internal budget")

    signal.signal(signal.SIGALRM, deadline)
    signal.alarm(LIMIT_SECONDS)
    try:
        evidence["base_commit"] = require_workflow()
        manage = load_source_tools()
        evidence["stage"] = "predecessor-source-verification"
        manage.verify(ROOT, metadata=True)
        evidence["stage"] = "fixed-official-archive"
        release = download()
        (evidence_dir / (IDENTITY + ".crate")).write_bytes(release.archive)
        evidence["original_files_sha256"] = release.inventory
        evidence["stage"] = "source-upgrade-and-existing-provenance-gates"
        mapping = upgrade(ROOT, release, manage, run, evidence)
        evidence["stage"] = "bounded-patch-output"
        evidence["patch"] = emit_patch(directory / "candidate.patch", mapping)
        evidence["status"] = "prepared-for-review"
        evidence["stage"] = "complete"
        print("Prepared one SQLite source upgrade patch; no compilation, tests, audit or publication.")
    except Exception as error:
        # Preserve every failed stage and re-raise; never emit a partial success.
        evidence["error"] = str(error)
        if isinstance(error, subprocess.CalledProcessError):
            evidence["command_error"] = error.stderr.decode("utf-8", errors="replace")[-16384:]
        (directory / "candidate.patch").unlink(missing_ok=True)
        raise
    finally:
        signal.alarm(0)
        evidence["elapsed_seconds"] = round(time.monotonic() - START, 3)
        (evidence_dir / "result.json").write_bytes(json_bytes(evidence))


if __name__ == "__main__":
    main()
