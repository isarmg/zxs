#!/usr/bin/env python3
"""Run bounded, paired benchmarks only on the authorized GitHub Actions carrier."""

from __future__ import annotations

import argparse
import errno
import hashlib
import json
import os
import platform
import re
import shutil
import signal
import stat
import subprocess
import sys
import time
import traceback
from pathlib import Path

from summarize import aggregate, summarize_driver
from tracing import parse_trace
from controller import load_plan, profile_settings
from verify_build import validate_precompiled
from growth import TIERS, admit, load_policy, resources


STAGES = ("build", "reopen", "recovery")
TRACE_CALLS = "read,pread64,readv,preadv,preadv2,write,pwrite64,writev,pwritev,pwritev2,fsync,fdatasync"
RSS_SETTLE_SECONDS = 0.5


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
    counts = {"files": 0, "directories": 0, "symlinks": 0, "apparent_bytes": 0, "allocated_bytes": 0,
              "metadata_object_files": 0, "replay_checkpoints": []}
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
                if entry.name == "replay-checkpoint.json" and "/metadata/" in entry.path:
                    counts["replay_checkpoints"].append({"path": str(Path(entry.path).relative_to(root)), "bytes": info.st_size})
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
        try:
            current_status = status_path.read_text(encoding="ascii")
        except (FileNotFoundError, ProcessLookupError):
            current_state = "disappeared"
        else:
            current_fields = dict(line.split(":", 1) for line in current_status.splitlines() if ":" in line)
            current_state = current_fields.get("State", "unknown").split()[0]
        return {"pid": pid, "valid": False, "memory": {}, "io": None,
                "rss_pending": current_state not in {"Z", "X", "disappeared"},
                "memory_error": {"reason": "missing_live_rss", "pid": pid, "path": str(status_path),
                                 "initial_state": state, "state": current_state,
                                 "initial_status": status,
                                 "rechecked_status": None if current_state == "disappeared" else current_status}}
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
            amount = int(line.rsplit(":", 1)[1])
            if amount < 0:
                raise ValueError("negative GNU time peak RSS")
            return amount * 1024
    return None


def final_budget_reason(existing: str | None, observed: dict, budget: argparse.Namespace, deadline: float) -> str | None:
    """Check final counters too: an exit between sampling polls is not an exemption."""
    if existing is not None:
        return existing
    if observed["finished_monotonic"] > deadline:
        return "cohort_wall_time_budget"
    if observed["wall_seconds"] > budget.phase_seconds:
        return "phase_wall_time_budget"
    if any(observed[key] is None for key in ("time_max_process_rss_bytes", "sample_allocated_bytes", "cohort_allocated_bytes", "filesystem_free_bytes")):
        return "final_resource_evidence_missing"
    maximum_rss = observed["time_max_process_rss_bytes"]
    if maximum_rss > budget.rss_mib * 1024 * 1024:
        return "time_max_process_rss_budget"
    if observed["sample_allocated_bytes"] > budget.disk_mib * 1024 * 1024:
        return "sample_allocated_disk_budget"
    if observed["cohort_allocated_bytes"] > budget.cohort_disk_mib * 1024 * 1024:
        return "cohort_allocated_disk_budget"
    if observed["filesystem_free_bytes"] < budget.minimum_free_mib * 1024 * 1024:
        return "filesystem_free_space_guard"
    return None


def failure_record(error: Exception) -> dict:
    return {"type": type(error).__name__, "message": str(error), "traceback": traceback.format_exc()}


def final_read(operation: str, read, errors: list[dict]):
    try:
        return read()
    except Exception as error:
        errors.append({"operation": operation, **failure_record(error)})
        return None


def supervise(command: list[str], sample: Path, stage: str, budget: argparse.Namespace, deadline: float) -> dict:
    """Retain evidence on all exits; external sampling is recorded as overhead."""
    started = time.monotonic()
    peak_sampled = 0
    reason = None
    disk_samples = []
    pending_rss = {}
    incomplete_rss_samples = 0
    process = None
    returncode = None
    monitor_error = None
    final_errors = []
    resource_file = sample / f"{stage}.resource.txt"
    invocation = ["/usr/bin/time", "-v", "-o", str(resource_file), "--", *command]
    try:
        (sample / f"{stage}.diskstats.before.txt").write_text(diskstats(), encoding="ascii")
        with (sample / f"{stage}.stdout.log").open("wb") as stdout, (sample / f"{stage}.stderr.log").open("wb") as stderr:
            process = subprocess.Popen(invocation, stdout=stdout, stderr=stderr, start_new_session=True)
            last_disk = started - budget.disk_poll_seconds
            with (sample / f"{stage}.process-samples.jsonl").open("w", encoding="utf-8") as snapshots:
                while process.poll() is None:
                    now = time.monotonic()
                    if any(now > since + RSS_SETTLE_SECONDS for since in pending_rss.values()):
                        raise ValueError(f"VmRSS resolution deadline exceeded for PIDs {sorted(pending_rss)}")
                    pids = set(process_tree(process.pid)) | pending_rss.keys()
                    observations = [snapshot for pid in sorted(pids) if (snapshot := process_snapshot(pid)) is not None]
                    observed_at = time.monotonic()
                    late_rss = [pid for pid, since in pending_rss.items() if observed_at > since + RSS_SETTLE_SECONDS]
                    pending_rss = {row["pid"]: pending_rss.get(row["pid"], now) for row in observations if row.get("rss_pending")}
                    incomplete_rss_samples += bool(pending_rss)
                    resident = sum(row["memory"].get("VmRSS_bytes", 0) for row in observations)
                    peak_sampled = max(peak_sampled, resident)
                    snapshots.write(json.dumps({"elapsed_seconds": now - started, "processes": observations,
                                                "rss_complete": not pending_rss,
                                                "observed_tree_rss_bytes": resident,
                                                "rss_pending_pids": sorted(pending_rss),
                                                "rss_late_pids": late_rss}) + "\n")
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
                    if late_rss:
                        raise ValueError(f"VmRSS resolution deadline exceeded during sampling for PIDs {late_rss}")
                    if any(time.monotonic() - since >= RSS_SETTLE_SECONDS for since in pending_rss.values()):
                        if process.poll() is None:
                            raise ValueError(f"VmRSS unavailable beyond {RSS_SETTLE_SECONDS}s for PIDs {sorted(pending_rss)}")
                        break
                    delay = budget.process_poll_seconds
                    if pending_rss:
                        delay = min(delay, max(0.0, min(pending_rss.values()) + RSS_SETTLE_SECONDS - time.monotonic()))
                    time.sleep(delay)
    except Exception as error:
        monitor_error = failure_record(error)
    finally:
        if process is not None:
            try:
                if process.poll() is None:
                    stop_process(process)
                returncode = process.wait()
            except Exception as error:
                final_errors.append({"operation": "process_cleanup", **failure_record(error)})
    # A late, unavailable sample is resolved only by an actual final observation,
    # never by the driver's completion marker or a fabricated zero RSS.
    final_rss = []
    for pid in list(pending_rss):
        errors_before = len(final_errors)
        observed = final_read(f"final_process_snapshot:{pid}", lambda pid=pid: process_snapshot(pid), final_errors)
        observed_at = time.monotonic()
        final_rss.append({"pid": pid, "observed_at": observed_at, "snapshot": observed,
                          "read_failed": len(final_errors) != errors_before})
        if observed_at > pending_rss[pid] + RSS_SETTLE_SECONDS and monitor_error is None:
            monitor_error = {"type": "RssSettleTimeout", "message": f"VmRSS resolution deadline exceeded for PID {pid}"}
        if len(final_errors) == errors_before and (observed is None or (not observed["valid"] and not observed.get("rss_pending"))):
            pending_rss.pop(pid)
    if pending_rss and monitor_error is None:
        monitor_error = {"type": "UnresolvedRss", "message": f"VmRSS still unavailable after wrapper exit for PIDs {sorted(pending_rss)}"}
    finished = time.monotonic()
    elapsed = finished - started
    final_read("diskstats_after", lambda: (sample / f"{stage}.diskstats.after.txt").write_text(diskstats(), encoding="ascii"), final_errors)
    final_retained = final_read("sample_inventory", lambda: inventory(sample), final_errors)
    final_cohort_retained = final_read("cohort_inventory", lambda: inventory(budget.output_root), final_errors)
    final_observation = {
        "finished_monotonic": finished, "wall_seconds": elapsed,
        "time_max_process_rss_bytes": final_read("gnu_time_peak_rss", lambda: read_peak_rss(resource_file), final_errors),
        "sample_allocated_bytes": None if final_retained is None else final_retained["allocated_bytes"],
        "cohort_allocated_bytes": None if final_cohort_retained is None else final_cohort_retained["allocated_bytes"],
        "filesystem_free_bytes": final_read("filesystem_free_space", lambda: shutil.disk_usage(sample).free, final_errors),
    }
    reason = final_budget_reason(reason, final_observation, budget, deadline)
    result = {
        "command": invocation, "returncode": returncode, "wall_seconds": elapsed,
        "budget_stop_reason": reason, "sampled_tree_peak_rss_bytes": peak_sampled,
        "time_max_process_rss_bytes": final_observation["time_max_process_rss_bytes"], "disk_samples": disk_samples,
        "final_budget_observation": final_observation,
        "resource_scope": "time max RSS is maximum process RSS; sampled tree RSS sums concurrently observed processes",
        "monitor_error": monitor_error, "finalization_errors": final_errors,
        "rss_pending_at_exit": sorted(pending_rss), "rss_settle_seconds": RSS_SETTLE_SECONDS,
        "rss_incomplete_samples": incomplete_rss_samples,
        "rss_final_observations": final_rss,
    }
    write_json(sample / f"{stage}.supervisor.json", result)
    return result


def run_stage(binary: Path, pool: Path, sample: Path, stage: str, operations: int, mode: str, budget: argparse.Namespace, deadline: float, label: str) -> dict:
    result_path = sample / f"{stage}.driver.jsonl"
    command = [str(binary), "--pool-root", str(pool), "--operations", str(operations), "--result", str(result_path), "--mode", mode, "--phase", stage]
    if getattr(budget, "scale_authorized", False):
        command.append("--scale-authorized")
    trace = sample / f"{stage}.strace.txt"
    if mode == "counts":
        command = ["strace", "-f", "-yy", "-s", "256", "-e", f"trace={TRACE_CALLS}", "-o", str(trace), "--", *command]
    supervised = supervise(command, sample, stage, budget, deadline)
    stage_result = {"supervisor": supervised}
    if trace.exists():
        stage_result["trace"] = parse_trace(trace, pool)
    if supervised["monitor_error"] or supervised["finalization_errors"]:
        stage_result.update(status="failed", reason="monitoring_error")
    elif supervised["budget_stop_reason"]:
        stage_result["status"] = "incomplete"
    elif supervised["returncode"] != 0:
        stage_result["status"] = "failed"
    else:
        try:
            stage_result["driver"] = summarize_driver(result_path, stage, operations, mode, baseline=label == "baseline",
                                                       required_protocol=getattr(budget, "query_protocol", None))
            stage_result["status"] = "passed"
        except (OSError, ValueError) as error:
            stage_result.update(status="failed", reason="invalid_driver_evidence", error=failure_record(error))
        if stage_result["status"] == "passed" and mode == "counts" and not stage_result["trace"]["complete"]:
            stage_result["status"] = "incomplete"
            stage_result["reason"] = "trace_accounting_incomplete"
    return stage_result


def run_sample(label: str, source: Path, binary: Path, operations: int, repeat: int, mode: str, args: argparse.Namespace, deadline: float, skip_reason: str | None = None) -> dict:
    name = f"{operations:06d}-{mode}-{repeat:02d}-{label}"
    sample = args.output_root / "samples" / name
    sample.mkdir(parents=True, exist_ok=False)
    pool = sample / "pool"
    binary_hash = sha256(binary)
    if binary_hash != args.provenances[label]["binary_sha256"]:
        raise ValueError(f"precompiled {label} binary changed after provenance validation")
    manifest = {
        "label": label, "sha": args.revisions[label], "source": str(source), "binary": str(binary),
        "binary_sha256": binary_hash, "profile": args.profile, "query_protocol": args.query_protocol,
        "operations": operations, "repeat": repeat,
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
    args.provenances = {}
    for label in args.revisions:
        source = getattr(args, label + "_source")
        head = command_output(["git", "rev-parse", "HEAD"], source)
        if head != args.revisions[label]:
            raise ValueError(f"{label} source HEAD mismatch: {head}")
        source_status = command_output(["git", "status", "--short"], source)
        if source_status:
            raise ValueError(f"{label} runtime source checkout must remain pristine")
        proof = validate_precompiled(getattr(args, label + "_provenance"), {
            "source_sha": head, "adapter": "baseline" if label == "baseline" else "current",
            "binary_name": "history-scale-driver", "binary_sha256": sha256(getattr(args, label + "_binary")),
            "driver_sha256": sha256(Path(__file__).with_name("driver.rs")),
            "lockfile_sha256": sha256(source / "Cargo.lock"),
            "manifest_sha256_before": sha256(source / "tools/perf-check/Cargo.toml"),
            "cargo_config_sha256": sha256(source / ".cargo/config.toml"),
            "toolchain_config_sha256": sha256(source / "rust-toolchain.toml"),
        })
        args.provenances[label] = proof
        if args.growth_review is not None and proof["binary_sha256"] != args.growth_policy_data["binary_sha256"]:
            raise ValueError("actual verified binary differs from the growth policy")
        validate_build_identity(proof["build_identity"], args.build_run_id, args.build_carrier_sha)
        sources[label] = {
            "head": head, "status": source_status,
            "tree": command_output(["git", "rev-parse", "HEAD^{tree}"], source),
            "lockfile_sha256": sha256(source / "Cargo.lock"),
            "precompiled_provenance": proof,
            "provenance_sha256": sha256(getattr(args, label + "_provenance")),
        }
    allowlist = ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTUP_TOOLCHAIN", "CARGO_INCREMENTAL", "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT", "GITHUB_SHA", "RUNNER_OS", "RUNNER_ARCH")
    if args.growth_review is not None and sources["candidate"]["tree"] != args.growth_policy_data["source_tree"]:
        raise ValueError("growth product tree differs from the reviewed source")
    return {
        "rustc": rustc, "cargo": command_output(["cargo", "+1.99.0", "--version"]),
        "observer_tools": {"python": sys.version, "strace": command_output(["strace", "--version"]).splitlines()[0],
                           "gnu_time": command_output(["/usr/bin/time", "--version"]).splitlines()[0]},
        "platform": platform.platform(), "cpu": command_output(["lscpu", "--json"]),
        "filesystem": command_output(["findmnt", "--json", "--target", str(args.output_root)]),
        "selected_environment": {key: os.environ[key] for key in allowlist if key in os.environ},
        "sources": sources, "profile": "release", "timing": "untraced; external process/RSS and live filesystem inventory sampling remain enabled",
        "sampling": {"process_seconds": args.process_poll_seconds, "disk_seconds": args.disk_poll_seconds},
        "process_cpu_affinity": sorted(os.sched_getaffinity(0)),
        "budgets": {"phase_seconds": args.phase_seconds, "cohort_seconds": args.cohort_seconds, "rss_mib": args.rss_mib, "disk_mib_per_sample": args.disk_mib, "cohort_disk_mib": args.cohort_disk_mib, "minimum_free_mib": args.minimum_free_mib},
        "io_scope": "process samples include harness oracle/result I/O; strace counters exclude pool/harness; host diskstats are shared device counters, not exclusively attributable physical I/O",
        "command": sys.argv,
        "acceptance_plan": args.plan_data, "acceptance_plan_sha256": sha256(args.plan),
        "validation_profile": args.profile, "scale_authorized": args.scale_authorized,
        "source_matrix": list(args.revisions),
        "query_protocol": args.query_protocol,
        "expected_build_identity": {"GITHUB_SHA": args.build_carrier_sha, "GITHUB_RUN_ID": args.build_run_id},
        "observer_identity": {key: os.environ.get(key) for key in ("GITHUB_SHA", "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT")},
        "growth_review": args.growth_review,
    }


def validate_build_identity(identity: dict, expected_run: str, expected_carrier: str) -> None:
    if identity["GITHUB_SHA"] != expected_carrier or identity["GITHUB_RUN_ID"] != expected_run:
        raise ValueError("precompiled binary does not belong to the explicitly selected build carrier/run")


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    for label in ("baseline", "candidate"):
        parser.add_argument(f"--{label}-binary", type=Path)
        parser.add_argument(f"--{label}-source", type=Path)
        parser.add_argument(f"--{label}-provenance", type=Path)
    parser.add_argument("--candidate-only", action="store_true")
    parser.add_argument("--plan", type=Path, required=True)
    parser.add_argument("--profile", choices=("small-selftest", "boundaries", "1000", "10000", *TIERS), required=True)
    parser.add_argument("--growth-policy", type=Path)
    parser.add_argument("--growth-evidence-root", type=Path)
    parser.add_argument("--prior-run-id")
    parser.add_argument("--prior-carrier-sha")
    parser.add_argument("--build-run-id", required=True)
    parser.add_argument("--build-carrier-sha", required=True)
    parser.add_argument("--output-root", type=Path, required=True)
    parser.add_argument("--rss-mib", type=int, default=8192)
    parser.add_argument("--disk-mib", type=int, default=12288)
    parser.add_argument("--cohort-disk-mib", type=int, default=24576)
    parser.add_argument("--minimum-free-mib", type=int, default=2048)
    parser.add_argument("--process-poll-seconds", type=float, default=0.1)
    parser.add_argument("--disk-poll-seconds", type=float, default=2.0)
    args = parser.parse_args()
    for label in (("candidate",) if args.candidate_only else ("baseline", "candidate")):
        for suffix in ("binary", "source", "provenance"):
            if getattr(args, label + "_" + suffix) is None:
                parser.error(f"--{label}-{suffix} is required for the selected source matrix")
    if args.candidate_only and any(getattr(args, "baseline_" + suffix) is not None for suffix in ("binary", "source", "provenance")):
        parser.error("current-only experiment does not accept baseline inputs")
    if args.candidate_only and args.profile not in {"small-selftest", "boundaries", *TIERS}:
        parser.error("current-only experiment is limited to selftest and boundaries")
    if re.fullmatch(r"[1-9][0-9]*", args.build_run_id) is None or re.fullmatch(r"[0-9a-f]{40}", args.build_carrier_sha) is None:
        parser.error("build reuse requires an exact positive run ID and lowercase 40-hex carrier SHA")
    args.plan = args.plan.resolve(strict=True)
    args.plan_data = load_plan(args.plan)
    args.growth_review = None
    if args.profile in TIERS:
        if not args.candidate_only or any(value is None for value in (args.growth_policy, args.growth_evidence_root, args.prior_run_id, args.prior_carrier_sha)):
            parser.error("growth requires current-only mode and complete predecessor evidence")
        args.growth_policy_data = load_policy(args.growth_policy.resolve(strict=True))
        if (args.build_run_id, args.build_carrier_sha) != (args.growth_policy_data["build_run_id"], args.growth_policy_data["build_carrier_sha"]):
            parser.error("growth must reuse the original reviewed binary build")
        settings, args.growth_review = admit(args.plan_data, args.profile, args.growth_policy_data,
                                           args.growth_evidence_root.resolve(strict=True), args.prior_run_id, args.prior_carrier_sha)
        for name, value in resources(args.profile).items():
            if getattr(args, name) != value:
                parser.error(f"growth requires its reviewed {name} budget: {value}")
    else:
        if any(value is not None for value in (args.growth_policy, args.growth_evidence_root, args.prior_run_id, args.prior_carrier_sha)):
            parser.error("predecessor growth evidence is only accepted by growth profiles")
        settings = profile_settings(args.plan_data, args.profile)
    for key in ("sizes", "repeats", "counts_repeats", "counts_sizes", "phase_seconds", "cohort_seconds"):
        setattr(args, key, settings[key])
    args.revisions = {"baseline": settings["baseline_sha"], "candidate": settings["candidate_sha"]}
    if args.candidate_only:
        args.revisions.pop("baseline")
    args.scale_authorized = args.profile != "small-selftest"
    args.query_protocol = "separate-outcome-health"
    if args.repeats < 0 or args.counts_repeats < 0 or args.repeats + args.counts_repeats < 1 or min(args.phase_seconds, args.cohort_seconds, args.rss_mib, args.disk_mib, args.cohort_disk_mib, args.minimum_free_mib, args.process_poll_seconds, args.disk_poll_seconds) <= 0:
        parser.error("positive budgets and timing repeats are required")
    for label in args.revisions:
        for suffix in ("binary", "source", "provenance"):
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
                for label in args.revisions:
                    requested["samples"].append({
                        "sample": f"{operations:06d}-{mode}-{repeat:02d}-{label}",
                        "label": label, "operations": operations, "repeat": repeat, "mode": mode,
                    })
    write_json(args.output_root / "requested.json", requested)
    observed_environment = environment(args)
    if args.growth_review is not None:
        free = shutil.disk_usage(args.output_root).free
        needed = args.growth_review["forecast"]["cohort_bytes"] + args.minimum_free_mib * 1048576
        args.growth_review["initial_free_bytes"] = free
        args.growth_review["required_free_bytes"] = needed
        if free < needed:
            args.growth_review["admitted"] = False
            args.growth_review["denial_reasons"].append("initial free space cannot retain forecast fresh pools plus reserve")
        write_json(args.output_root / "growth-review.json", args.growth_review)
    write_json(args.output_root / "environment.json", observed_environment)
    if args.growth_review is not None:
        if not args.growth_review["admitted"]:
            write_json(args.output_root / "summary.json", growth_summary(args))
            print(json.dumps(args.growth_review, sort_keys=True), flush=True)
            return 2
    for label, proof in args.provenances.items():
        write_json(args.output_root / f"{label}-precompiled-provenance.json", proof)
    args.cohort_started = time.monotonic()
    deadline = args.cohort_deadline = args.cohort_started + args.cohort_seconds
    halted = {}
    for mode, repeats in (("timing", args.repeats), ("counts", args.counts_repeats)):
        for repeat in range(1, repeats + 1):
            labels = list(args.revisions) if repeat % 2 else list(reversed(args.revisions))
            for operations in args.sizes if mode == "timing" else args.counts_sizes:
                for label in labels:
                    key = (label, operations)
                    sample = run_sample(label, getattr(args, label + "_source"), getattr(args, label + "_binary"), operations, repeat, mode, args, deadline, halted.get(key))
                    if sample["status"] in {"failed", "incomplete"}:
                        halted[key] = "earlier_sample_failed_or_incomplete_under_same_budget"
                    complete = growth_summary(args)
                    write_json(args.output_root / "summary.json", complete)
                    if args.growth_review is not None:
                        if complete.get("growth_final_budget_reasons") or complete.get("growth_cohort_budget_exceeded"):
                            halted[key] = "growth_final_cohort_budget"
    requested["cohort_complete"] = True
    write_json(args.output_root / "requested.json", requested)
    summary = growth_summary(args)
    write_json(args.output_root / "summary.json", summary)
    if summary["status_counts"].get("failed", 0):
        return 1
    return 0 if summary["all_requested_samples_passed"] else 2


def growth_summary(args: argparse.Namespace) -> dict:
    summary = aggregate(args.output_root)
    if args.growth_review is not None:
        summary["coverage"] = args.growth_review["coverage"]
        summary["all_growth_capacity_metrics_passed"] = False  # One run never certifies the entire growth/capacity scope.
        summary["growth_admitted"] = args.growth_review["admitted"]
        if getattr(args, "cohort_deadline", None) is not None:
            reserve = 16 * 1048576  # Room for final manifests/summary; never a fabricated observed byte count.
            sample_bytes = max((row["total_sample_inventory"]["allocated_bytes"] for row in summary["samples"]), default=0)
            cohort_bytes = inventory(args.output_root)["allocated_bytes"]
            free_bytes = shutil.disk_usage(args.output_root).free
            reasons = []
            if sample_bytes + reserve > args.disk_mib * 1048576:
                reasons.append("final_complete_sample_disk_budget")
            if cohort_bytes + reserve > args.cohort_disk_mib * 1048576:
                reasons.append("final_complete_cohort_disk_budget")
            if free_bytes < args.minimum_free_mib * 1048576 + reserve:
                reasons.append("final_free_space_guard")
            summary["growth_final_resource_observation"] = {"maximum_complete_sample_bytes": sample_bytes,
                "complete_cohort_bytes": cohort_bytes, "filesystem_free_bytes": free_bytes, "final_metadata_reserve_bytes": reserve}
            summary["growth_final_budget_reasons"] = reasons
            now = time.monotonic()
            summary["cohort_wall_seconds"] = now - args.cohort_started
            summary["growth_cohort_budget_exceeded"] = now > args.cohort_deadline
            if summary["growth_cohort_budget_exceeded"] or reasons:
                summary["all_requested_samples_passed"] = False
    return summary


if __name__ == "__main__":
    raise SystemExit(main())
