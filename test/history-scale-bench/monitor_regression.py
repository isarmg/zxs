#!/usr/bin/env python3
"""GitHub-only reproduction of the observed 1,025-record recovery monitor race."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
import time
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools" / "history-scale-bench"))
from run import REVISIONS, command_output, inventory, run_stage, sha256, write_json  # noqa: E402


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.binary = args.binary.resolve(strict=True)
    args.source = args.source.resolve(strict=True)
    args.output = args.output.resolve()
    args.output_root = args.output
    args.phase_seconds = 600
    args.cohort_seconds = 2400
    args.rss_mib = 8192
    args.disk_mib = 12288
    args.cohort_disk_mib = 24576
    args.minimum_free_mib = 2048
    args.process_poll_seconds = 0.1
    args.disk_poll_seconds = 2.0
    return args


def main() -> int:
    if os.environ.get("GITHUB_ACTIONS") != "true":
        raise SystemExit("Monitor regression execution is restricted to GitHub Actions.")
    args = arguments()
    revision = command_output(["git", "rev-parse", "HEAD"], args.source)
    labels = [label for label, sha in REVISIONS.items() if sha == revision]
    if len(labels) != 1:
        raise ValueError(f"source is not an authorized exact benchmark revision: {revision}")
    label = labels[0]
    rustc = command_output(["rustc", "+1.99.0", "--version", "--verbose"])
    if rustc.splitlines()[0].split()[1] != "1.99.0":
        raise ValueError("monitor regression requires exact Rust 1.99.0")
    if shutil.which("strace") is None or not Path("/usr/bin/time").is_file():
        raise ValueError("strace and GNU /usr/bin/time are required")
    args.output.mkdir(parents=True, exist_ok=False)
    pool = args.output / "pool"
    stages = (("build", "timing", "fixture"), ("reopen", "timing", "fixture"), ("recovery", "counts", "measurement"))
    manifest = {
        "purpose": "monitor regression for the original 1025-record recovery EACCES failure",
        "label": label, "source_revision": revision, "binary": str(args.binary),
        "binary_sha256": sha256(args.binary), "driver_sha256": sha256(args.source / "tools/history-scale-bench/driver.rs"),
        "operations": 1025, "status": "running", "rustc": rustc,
        "source_status": command_output(["git", "status", "--short"], args.source),
        "stages": {stage: {"status": "not_executed", "reason": "prerequisite_not_completed", "mode": mode, "role": role} for stage, mode, role in stages},
        "budgets": {"phase_seconds": args.phase_seconds, "cohort_seconds": args.cohort_seconds, "rss_mib": args.rss_mib, "sample_disk_mib": args.disk_mib, "cohort_disk_mib": args.cohort_disk_mib, "minimum_free_mib": args.minimum_free_mib},
        "limits": [
            "Timing build/reopen stages prepare and verify the new private fixture; they are not replacements for the original scale samples.",
            "Only recovery uses syscall tracing; the trace includes strict semantic verification and retries.",
            "The original failed boundary sample remains failed. This independent regression does not rewrite it.",
            "An IO permission denial is tolerated only with a fresh, explicit observation proving process exit; the diagnostic retains no valid memory or IO measurement.",
            "Logical member directories share one runner filesystem; device statistics are not independent physical disk evidence.",
        ],
    }
    write_json(args.output / "manifest.json", manifest)
    deadline = time.monotonic() + args.cohort_seconds
    try:
        for index, (stage, mode, role) in enumerate(stages):
            try:
                result = run_stage(args.binary, pool, args.output, stage, 1025, mode, args, deadline, label)
            except (OSError, ValueError, json.JSONDecodeError) as error:
                result = {"status": "failed", "reason": f"{type(error).__name__}: {error}"}
            result.update(mode=mode, role=role)
            manifest["stages"][stage] = result
            write_json(args.output / "manifest.json", manifest)
            if result["status"] != "passed":
                manifest["status"] = result["status"]
                for remaining, _, _ in stages[index + 1:]:
                    manifest["stages"][remaining]["reason"] = f"prerequisite_{stage}_{result['status']}"
                break
        else:
            manifest["status"] = "passed"
    finally:
        harness = pool / "harness"
        if harness.is_dir():
            try:
                shutil.copytree(harness, args.output / "oracle-evidence")
            except OSError as error:
                manifest.update(status="failed", oracle_copy_error=f"{type(error).__name__}: {error}")
        manifest["product_inventory"] = inventory(pool, exclude_harness=True)
        manifest["total_inventory"] = inventory(args.output)
        write_json(args.output / "manifest.json", manifest)
    print(json.dumps({"purpose": "monitor_regression", "label": label, "status": manifest["status"]}), flush=True)
    if manifest["status"] == "passed":
        return 0
    return 2 if manifest["status"] == "incomplete" else 1


if __name__ == "__main__":
    raise SystemExit(main())
