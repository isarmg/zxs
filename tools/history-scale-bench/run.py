#!/usr/bin/env python3
"""Run bounded, paired benchmarks only on the authorized GitHub Actions carrier."""

from __future__ import annotations

import argparse
import errno
import hashlib
import json
import os
import platform
import shutil
import signal
import stat
import subprocess
import sys
import time
from pathlib import Path

from summarize import aggregate, summarize_driver
from tracing import parse_trace


REVISIONS = {
    "baseline": "e2f33d9882cf7263412d9370b6a703e0b216357d",
    "candidate": "13b664e9fd41a356ffb7ed982d21e97abb123626",
}
STAGES = ("build", "reopen", "recovery")
TRACE_CALLS = "read,pread64,readv,preadv,preadv2,write,pwrite64,writev,pwritev,pwritev2,fsync,fdatasync"


def write_json(path: Path, value: dict) -> None:
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    temporary.replace(path)


def command_output(command: list[str], cwd: Path | None = None) -> str:
    return subprocess.check_output(command, cwd=cwd, text=True, stderr=subprocess.STDOUT).strip()


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def inventory(root: Path, *, exclude_harness: bool = False) -> dict:
    counts = {"files": 0, "directories": 0, "symlinks": 0, "apparent_bytes": 0, "allocated_bytes": 0, "metadata_object_files": 0}
    pending = [root]
    while pending:
        directory = pending.pop()
        try:
            entries = list(os.scandir(directory))
        except FileNotFoundError:
            continue  # Files may disappear during the explicitly sampled live inventory.
        for entry in entries:
            if exclude_harness and directory == root and entry.name == "harness":
                continue
            try:
                info = entry.stat(follow_symlinks=False)
            except FileNotFoundError:
                continue
            counts["apparent_bytes"] += info.st_size
            counts["allocated_bytes"] += info.st_blocks * 512
            if stat.S_ISDIR(info.st_mode):
                counts["directories"] += 1
                pending.append(Path(entry.path))
            elif stat.S_ISLNK(info.st_mode):
                counts["symlinks"] += 1
            elif stat.S_ISREG(info.st_mode):
                counts["files"] += 1
                if "/metadata/objects/" in entry.path:
                    counts["metadata_object_files"] += 1
    return counts


def process_tree(root_pid: int) -> list[int]:
    pids, pending = [], [root_pid]
    while pending:
        pid = pending.pop()
        pids.append(pid)
        try:
            children = Path(f"/proc/{pid}/task/{pid}/children").read_text(encoding="ascii")
        except FileNotFoundError:
            continue
        pending.extend(int(child) for child in children.split())
    return pids


def process_snapshot(pid: int) -> dict | None:
    status_path = Path(f"/proc/{pid}/status")
    io_path = Path(f"/proc/{pid}/io")
    try:
        status = status_path.read_text(encoding="ascii")
    except (FileNotFoundError, ProcessLookupError):
        return None
    fields = {key: value.strip() for key, _, value in (line.partition(":") for line in status.splitlines())}
    state = fields["State"].split()[0]
    if state in {"Z", "X"}:
        return {"pid": pid, "valid": False, "memory": {}, "io": None,
                "process_exit": {"pid": pid, "path": str(status_path), "errno": None, "state": state}}
    memory = {}
    for key, value in fields.items():
        if key in {"VmRSS", "VmHWM", "VmSize"}:
            amount, unit = value.split()
            if unit != "kB" or int(amount) < 0:
                raise ValueError(f"invalid {key} in {status_path}")
            memory[key + "_bytes"] = int(amount) * 1024
    if "VmRSS_bytes" not in memory:
        raise ValueError(f"missing live process VmRSS in {status_path}")
    try:
        io_text = io_path.read_text(encoding="ascii")
    except (FileNotFoundError, ProcessLookupError):
        return None
    except OSError as error:
        if error.errno not in {errno.EACCES, errno.EPERM}:
            raise
        try:
            current_status = status_path.read_text(encoding="ascii")
        except (FileNotFoundError, ProcessLookupError):
            current_state = "disappeared"
        else:
            current_fields = dict(line.split(":", 1) for line in current_status.splitlines() if ":" in line)
            current_state = current_fields.get("State", "unknown").split()[0]
        if current_state not in {"Z", "X", "disappeared"}:
            raise
        return {"pid": pid, "valid": False, "memory": {}, "io": None,
                "io_error": {"errno": error.errno, "path": str(io_path), "pid": pid, "state": current_state}}
    io_counters = {key: int(value) for key, value in (line.split(":", 1) for line in io_text.splitlines())}
    return {"pid": pid, "valid": True, "memory": memory, "io": io_counters}


def diskstats() -> str:
    return Path("/proc/diskstats").read_text(encoding="ascii")


def stop_process(process: subprocess.Popen) -> None:
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        process.wait(timeout=0.5)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait()


def read_peak_rss(path: Path) -> int | None:
    if not path.exists():
        return None
    for line in path.read_text(encoding="utf-8").splitlines():
        if "Maximum resident set size (kbytes):" in line:
            return int(line.rsplit(":", 1)[1]) * 1024
    return None


def final_budget_reason(existing: str | None, observed: dict, budget: argparse.Namespace, deadline: float) -> str | None:
    """Check final counters too: an exit between sampling polls is not an exemption."""
    if existing is not None:
        return existing
    if observed["finished_monotonic"] > deadline:
        return "cohort_wall_time_budget"
    if observed["wall_seconds"] > budget.phase_seconds:
        return "phase_wall_time_budget"
    maximum_rss = observed["time_max_process_rss_bytes"]
    if maximum_rss is None:
        return "final_resource_evidence_missing"
    if maximum_rss > budget.rss_mib * 1024 * 1024:
        return "time_max_process_rss_budget"
    if observed["sample_allocated_bytes"] > budget.disk_mib * 1024 * 1024:
        return "sample_allocated_disk_budget"
    if observed["cohort_allocated_bytes"] > budget.cohort_disk_mib * 1024 * 1024:
        return "cohort_allocated_disk_budget"
    if observed["filesystem_free_bytes"] < budget.minimum_free_mib * 1024 * 1024:
        return "filesystem_free_space_guard"
    return None


def supervise(command: list[str], sample: Path, stage: str, budget: argparse.Namespace, deadline: float) -> dict:
    """Retain evidence on all exits; external sampling is recorded as overhead."""
    started = time.monotonic()
    peak_sampled = 0
    reason = None
    disk_samples = []
    (sample / f"{stage}.diskstats.before.txt").write_text(diskstats(), encoding="ascii")
    resource_file = sample / f"{stage}.resource.txt"
    invocation = ["/usr/bin/time", "-v", "-o", str(resource_file), "--", *command]
    with (sample / f"{stage}.stdout.log").open("wb") as stdout, (sample / f"{stage}.stderr.log").open("wb") as stderr:
        process = subprocess.Popen(invocation, stdout=stdout, stderr=stderr, start_new_session=True)
        last_disk = 0.0
        try:
            with (sample / f"{stage}.process-samples.jsonl").open("w", encoding="utf-8") as snapshots:
                while process.poll() is None:
                    now = time.monotonic()
                    observations = [snapshot for pid in process_tree(process.pid) if (snapshot := process_snapshot(pid)) is not None]
                    resident = sum(row["memory"].get("VmRSS_bytes", 0) for row in observations)
                    peak_sampled = max(peak_sampled, resident)
                    snapshots.write(json.dumps({"elapsed_seconds": now - started, "processes": observations}) + "\n")
                    if now - started > budget.phase_seconds or now > deadline:
                        reason = "phase_wall_time_budget" if now <= deadline else "cohort_wall_time_budget"
                    elif resident > budget.rss_mib * 1024 * 1024:
                        reason = "sampled_process_tree_rss_budget"
                    if now - last_disk >= budget.disk_poll_seconds:
                        retained = inventory(sample)
                        cohort_retained = inventory(budget.output_root)
                        free_bytes = shutil.disk_usage(sample).free
                        disk_samples.append({"elapsed_seconds": now - started, "cohort_allocated_bytes": cohort_retained["allocated_bytes"], "filesystem_free_bytes": free_bytes, **retained})
                        last_disk = now
                        if reason is None:
                            if retained["allocated_bytes"] > budget.disk_mib * 1024 * 1024:
                                reason = "sample_allocated_disk_budget"
                            elif cohort_retained["allocated_bytes"] > budget.cohort_disk_mib * 1024 * 1024:
                                reason = "cohort_allocated_disk_budget"
                            elif free_bytes < budget.minimum_free_mib * 1024 * 1024:
                                reason = "filesystem_free_space_guard"
                    if reason:
                        stop_process(process)
                        break
                    time.sleep(budget.process_poll_seconds)
        finally:
            if process.poll() is None:
                stop_process(process)
        returncode = process.wait()
    finished = time.monotonic()
    elapsed = finished - started
    (sample / f"{stage}.diskstats.after.txt").write_text(diskstats(), encoding="ascii")
    final_retained = inventory(sample)
    final_cohort_retained = inventory(budget.output_root)
    final_observation = {
        "finished_monotonic": finished, "wall_seconds": elapsed,
        "time_max_process_rss_bytes": read_peak_rss(resource_file),
        "sample_allocated_bytes": final_retained["allocated_bytes"],
        "cohort_allocated_bytes": final_cohort_retained["allocated_bytes"],
        "filesystem_free_bytes": shutil.disk_usage(sample).free,
    }
    reason = final_budget_reason(reason, final_observation, budget, deadline)
    result = {
        "command": invocation, "returncode": returncode, "wall_seconds": elapsed,
        "budget_stop_reason": reason, "sampled_tree_peak_rss_bytes": peak_sampled,
        "time_max_process_rss_bytes": final_observation["time_max_process_rss_bytes"], "disk_samples": disk_samples,
        "final_budget_observation": final_observation,
        "resource_scope": "time max RSS is maximum process RSS; sampled tree RSS sums concurrently observed processes",
    }
    write_json(sample / f"{stage}.supervisor.json", result)
    return result


def run_stage(binary: Path, pool: Path, sample: Path, stage: str, operations: int, mode: str, budget: argparse.Namespace, deadline: float, label: str) -> dict:
    result_path = sample / f"{stage}.driver.jsonl"
    command = [str(binary), "--pool-root", str(pool), "--operations", str(operations), "--result", str(result_path), "--mode", mode, "--phase", stage]
    trace = sample / f"{stage}.strace.txt"
    if mode == "counts":
        command = ["strace", "-f", "-yy", "-s", "256", "-e", f"trace={TRACE_CALLS}", "-o", str(trace), "--", *command]
    supervised = supervise(command, sample, stage, budget, deadline)
    stage_result = {"supervisor": supervised}
    if trace.exists():
        stage_result["trace"] = parse_trace(trace, pool)
    if supervised["budget_stop_reason"]:
        stage_result["status"] = "incomplete"
    elif supervised["returncode"] != 0:
        stage_result["status"] = "failed"
    else:
        stage_result["driver"] = summarize_driver(result_path, stage, operations, mode, baseline=label == "baseline")
        stage_result["status"] = "passed"
        if mode == "counts" and not stage_result["trace"]["complete"]:
            stage_result["status"] = "incomplete"
            stage_result["reason"] = "trace_accounting_incomplete"
    return stage_result


def run_sample(label: str, source: Path, binary: Path, operations: int, repeat: int, mode: str, args: argparse.Namespace, deadline: float, skip_reason: str | None = None) -> dict:
    name = f"{operations:06d}-{mode}-{repeat:02d}-{label}"
    sample = args.output_root / "samples" / name
    sample.mkdir(parents=True, exist_ok=False)
    pool = sample / "pool"
    manifest = {
        "label": label, "sha": REVISIONS[label], "source": str(source), "binary": str(binary),
        "binary_sha256": sha256(binary), "operations": operations, "repeat": repeat,
        "mode": mode, "status": "running", "stages": {
            stage: {"status": "not_executed", "reason": "prerequisite_not_completed"}
            for stage in STAGES
        }, "pool": str(pool),
    }
    write_json(sample / "manifest.json", manifest)
    if skip_reason or time.monotonic() >= deadline:
        manifest.update(status="not_executed", reason=skip_reason or "cohort_wall_time_budget")
        for result in manifest["stages"].values():
            result["reason"] = manifest["reason"]
    else:
        try:
            for stage in STAGES:
                result = run_stage(binary, pool, sample, stage, operations, mode, args, deadline, label)
                manifest["stages"][stage] = result
                write_json(sample / "manifest.json", manifest)
                if result["status"] != "passed":
                    manifest["status"] = result["status"]
                    for remaining in STAGES[STAGES.index(stage) + 1:]:
                        manifest["stages"][remaining]["reason"] = f"prerequisite_{stage}_{result['status']}"
                    break
            else:
                manifest["status"] = "passed"
        except (OSError, ValueError, json.JSONDecodeError) as error:
            manifest.update(status="failed", reason=f"{type(error).__name__}: {error}")
            manifest["stages"][stage] = {"status": "failed", "reason": manifest["reason"]}
            for remaining in STAGES[STAGES.index(stage) + 1:]:
                manifest["stages"][remaining]["reason"] = f"prerequisite_{stage}_failed"
    harness = pool / "harness"
    if harness.is_dir():
        try:
            shutil.copytree(harness, sample / "oracle-evidence")
        except OSError as error:
            manifest.update(status="failed", oracle_copy_error=f"{type(error).__name__}: {error}")
    manifest["product_inventory"] = inventory(pool, exclude_harness=True)
    manifest["total_sample_inventory"] = inventory(sample)
    write_json(sample / "manifest.json", manifest)
    print(json.dumps({"sample": name, "status": manifest["status"]}), flush=True)
    return manifest


def environment(args: argparse.Namespace) -> dict:
    rustc = command_output(["rustc", "+1.99.0", "--version", "--verbose"])
    if rustc.splitlines()[0].split()[1] != "1.99.0":
        raise ValueError("formal benchmark requires exact Rust 1.99.0")
    sources = {}
    for label in REVISIONS:
        source = getattr(args, label + "_source")
        head = command_output(["git", "rev-parse", "HEAD"], source)
        if head != REVISIONS[label]:
            raise ValueError(f"{label} source HEAD mismatch: {head}")
        sources[label] = {
            "head": head, "status": command_output(["git", "status", "--short"], source),
            "lockfile_sha256": sha256(source / "Cargo.lock"),
        }
    allowlist = ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTUP_TOOLCHAIN", "CARGO_INCREMENTAL", "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT", "GITHUB_SHA", "RUNNER_OS", "RUNNER_ARCH")
    return {
        "rustc": rustc, "cargo": command_output(["cargo", "+1.99.0", "--version"]),
        "platform": platform.platform(), "cpu": command_output(["lscpu", "--json"]),
        "filesystem": command_output(["findmnt", "--json", "--target", str(args.output_root)]),
        "selected_environment": {key: os.environ[key] for key in allowlist if key in os.environ},
        "sources": sources, "profile": "release", "timing": "untraced; external process/RSS and live filesystem inventory sampling remain enabled",
        "sampling": {"process_seconds": args.process_poll_seconds, "disk_seconds": args.disk_poll_seconds},
        "budgets": {"phase_seconds": args.phase_seconds, "cohort_seconds": args.cohort_seconds, "rss_mib": args.rss_mib, "disk_mib_per_sample": args.disk_mib, "cohort_disk_mib": args.cohort_disk_mib, "minimum_free_mib": args.minimum_free_mib},
        "io_scope": "process samples include harness oracle/result I/O; strace counters exclude pool/harness; host diskstats are shared device counters, not exclusively attributable physical I/O",
        "command": sys.argv,
    }


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    for label in REVISIONS:
        parser.add_argument(f"--{label}-binary", type=Path, required=True)
        parser.add_argument(f"--{label}-source", type=Path, required=True)
    parser.add_argument("--output-root", type=Path, required=True)
    parser.add_argument("--sizes", default="255,256,257,1023,1024,1025,1000,10000,100000")
    parser.add_argument("--repeats", type=int, default=2)
    parser.add_argument("--counts-repeats", type=int, default=1)
    parser.add_argument("--counts-sizes", default=None, help="Optional subset; omitted sizes are explicitly not requested for tracing")
    parser.add_argument("--phase-seconds", type=float, default=900)
    parser.add_argument("--cohort-seconds", type=float, default=5400)
    parser.add_argument("--rss-mib", type=int, default=8192)
    parser.add_argument("--disk-mib", type=int, default=12288)
    parser.add_argument("--cohort-disk-mib", type=int, default=24576)
    parser.add_argument("--minimum-free-mib", type=int, default=2048)
    parser.add_argument("--process-poll-seconds", type=float, default=0.1)
    parser.add_argument("--disk-poll-seconds", type=float, default=2.0)
    args = parser.parse_args()
    args.sizes = [int(value) for value in args.sizes.split(",")]
    args.counts_sizes = args.sizes if args.counts_sizes is None else [int(value) for value in args.counts_sizes.split(",")]
    if not args.sizes or len(set(args.sizes)) != len(args.sizes) or any(size < 2 or size > 100000 for size in args.sizes):
        parser.error("sizes must be unique record targets in 2..100000")
    if not set(args.counts_sizes).issubset(args.sizes):
        parser.error("counts-sizes must be a subset of sizes")
    if args.repeats < 1 or args.counts_repeats < 0 or min(args.phase_seconds, args.cohort_seconds, args.rss_mib, args.disk_mib, args.cohort_disk_mib, args.minimum_free_mib, args.process_poll_seconds, args.disk_poll_seconds) <= 0:
        parser.error("positive budgets and timing repeats are required")
    for label in REVISIONS:
        for suffix in ("binary", "source"):
            setattr(args, label + "_" + suffix, getattr(args, label + "_" + suffix).resolve(strict=True))
    args.output_root = args.output_root.resolve()
    return args


def main() -> int:
    if os.environ.get("GITHUB_ACTIONS") != "true":
        raise SystemExit("Formal benchmark execution is restricted to GitHub Actions; local/cloud workspace is edit-only.")
    args = arguments()
    if not shutil.which("strace") or not Path("/usr/bin/time").is_file():
        raise SystemExit("strace and GNU /usr/bin/time are required; no measurement fallback")
    args.output_root.mkdir(parents=True, exist_ok=False)
    requested = {"cohort_complete": False, "samples": []}
    for mode, repeats in (("timing", args.repeats), ("counts", args.counts_repeats)):
        for repeat in range(1, repeats + 1):
            for operations in args.sizes if mode == "timing" else args.counts_sizes:
                for label in REVISIONS:
                    requested["samples"].append({
                        "sample": f"{operations:06d}-{mode}-{repeat:02d}-{label}",
                        "label": label, "operations": operations, "repeat": repeat, "mode": mode,
                    })
    write_json(args.output_root / "requested.json", requested)
    write_json(args.output_root / "environment.json", environment(args))
    deadline = time.monotonic() + args.cohort_seconds
    halted = {}
    for mode, repeats in (("timing", args.repeats), ("counts", args.counts_repeats)):
        for repeat in range(1, repeats + 1):
            labels = list(REVISIONS) if repeat % 2 else list(reversed(REVISIONS))
            for operations in args.sizes if mode == "timing" else args.counts_sizes:
                for label in labels:
                    key = (label, operations)
                    sample = run_sample(label, getattr(args, label + "_source"), getattr(args, label + "_binary"), operations, repeat, mode, args, deadline, halted.get(key))
                    if sample["status"] in {"failed", "incomplete"}:
                        halted[key] = "earlier_sample_failed_or_incomplete_under_same_budget"
                    write_json(args.output_root / "summary.json", aggregate(args.output_root))
    requested["cohort_complete"] = True
    write_json(args.output_root / "requested.json", requested)
    summary = aggregate(args.output_root)
    write_json(args.output_root / "summary.json", summary)
    if summary["status_counts"].get("failed", 0):
        return 1
    return 0 if summary["all_requested_samples_passed"] else 2


if __name__ == "__main__":
    raise SystemExit(main())
