#!/usr/bin/env python3
"""Capture an existing Development probe; never builds or accepts a pool/device path."""
import argparse
import datetime
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import shutil
import signal
import subprocess
import time


def digest(path):
    result = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            result.update(block)
    return result.hexdigest()


def write_json(path, value):
    with path.open("x", encoding="utf-8") as target:
        json.dump(value, target, indent=2, ensure_ascii=False, allow_nan=False)
        target.write("\n")


def optional_text(path):
    try:
        return {"text": Path(path).read_text(), "error": None}
    except OSError as error:
        return {"text": None, "error": str(error)}


def source_state(repo):
    def git(*args):
        return subprocess.check_output(["git", "-C", str(repo), *args])

    patch = git("diff", "--binary", "HEAD", "--")
    return {
        "commit": git("rev-parse", "HEAD").decode().strip(),
        "tree": git("rev-parse", "HEAD^{tree}").decode().strip(),
        "tracked_diff_sha256": hashlib.sha256(patch).hexdigest(),
        "cargo_lock_sha256": digest(repo / "Cargo.lock"),
        "toolchain_sha256": digest(repo / "rust-toolchain.toml"),
    }, patch


def rss_bytes(pid):
    try:
        status = Path(f"/proc/{pid}/status").read_text()
    except FileNotFoundError:
        return None
    for line in status.splitlines():
        if line.startswith("VmRSS:"):
            fields = line.split()
            if len(fields) != 3 or fields[2] != "kB":
                raise ValueError(f"invalid VmRSS field: {line}")
            return int(fields[1]) * 1024
    # A process that exited between wait4 and /proc can have no RSS field.
    return None


def stop(process):
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    _, status, usage = os.wait4(process.pid, 0)
    process.returncode = os.waitstatus_to_exitcode(status)
    return usage


def run(args):
    repo = args.repo.resolve(strict=True)
    binary = args.binary.resolve(strict=True)
    if args.workload == "namespace":
        if args.history is not None or args.namespace_files is None or args.namespace_case is None:
            raise ValueError("namespace requires --namespace-files and --namespace-case, without --history")
        if not 4 <= args.namespace_files <= 256:
            raise ValueError("namespace files must be 4..256")
        probe_args = [str(args.namespace_files), args.namespace_case]
    elif args.history is None:
        raise ValueError("this workload requires --history")
    elif args.workload == "probe":
        if not 4 <= args.history <= 1024:
            raise ValueError("probe history must be 4..=1024")
        probe_args = [str(args.history)]
    elif args.workload.startswith("phases-"):
        if not 1 <= args.history <= 128:
            raise ValueError("phase iterations must be 1..128")
        probe_args = [args.workload.removeprefix("phases-"), str(args.history), str(args.payload_bytes)]
    else:
        if args.history not in (64, 256, 1024):
            raise ValueError("recovery history must be 64, 256 or 1024")
        probe_args = [args.workload.removeprefix("recovery-"), str(args.history)]
    if (not math.isfinite(args.timeout_seconds) or args.timeout_seconds <= 0
            or args.rss_limit_bytes <= 0 or args.min_free_bytes <= 0):
        raise ValueError("experiment timeout, RSS limit and free-space reserve must be positive")
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    scratch = out / "scratch"
    scratch.mkdir()
    source_before, patch = source_state(repo)
    (out / "source.patch").write_bytes(patch)
    binary_hash = digest(binary)
    frozen = out / "probe-binary"
    shutil.copy2(binary, frozen)
    if digest(frozen) != binary_hash or digest(binary) != binary_hash:
        raise ValueError("binary changed while freezing; rebuild/capture sequentially")
    if args.build_receipt is not None:
        shutil.copyfile(args.build_receipt, out / "build-receipt.json")
    shutil.copyfile(Path(__file__).resolve(), out / "capture-script.py")
    files = ("/proc/cpuinfo", "/proc/meminfo", "/proc/self/mountinfo", "/etc/os-release",
             "/sys/fs/cgroup/cpu.max", "/sys/fs/cgroup/cpu.stat",
             "/sys/fs/cgroup/memory.max", "/sys/fs/cgroup/memory.current",
             "/sys/fs/cgroup/memory.events", "/sys/fs/cgroup/io.stat")
    metadata = {
        "started_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "execution_environment": "cloud-development-proxy-experiment",
        "physical_redundancy_evidence": False,
        "target_host_measurement": False,
        "source": source_before,
        "capture_script_sha256": digest(Path(__file__).resolve()),
        "binary_source_path": str(binary), "binary_sha256": binary_hash,
        "frozen_binary_path": str(frozen),
        "build_command_declared": args.build_command,
        "profile_declared": args.profile, "target_declared": args.target,
        "features_declared": args.features,
        "build_receipt_sha256": digest(out / "build-receipt.json") if args.build_receipt else None,
        "build_binding": "central build receipt/operator declaration; hashes observed here",
        "uname": list(platform.uname()),
        "cpu_affinity": sorted(os.sched_getaffinity(0)),
        "observations_before": {path: optional_text(path) for path in files},
        "build_environment": {key: os.environ.get(key) for key in (
            "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET",
            "CARGO_PROFILE_RELEASE_OPT_LEVEL", "CARGO_PROFILE_RELEASE_DEBUG",
            "CARGO_PROFILE_RELEASE_LTO", "CARGO_PROFILE_RELEASE_CODEGEN_UNITS")},
        "workload": args.workload, "history": args.history,
        "namespace_files": args.namespace_files if args.workload == "namespace" else None,
        "namespace_case": args.namespace_case if args.workload == "namespace" else None,
        "payload_bytes": args.payload_bytes if args.workload.startswith("phases-") else None,
        "concurrency": "one probe process; existing fixture read thread only",
        "cache_state": "new fixture; OS cache uncontrolled; no cache dropping; first open follows writes",
        "argv": [str(frozen), *probe_args],
        "scratch": str(scratch), "free_bytes_before": shutil.disk_usage(scratch).free,
        "timeout_seconds": args.timeout_seconds, "rss_limit_bytes": args.rss_limit_bytes,
        "min_free_bytes": args.min_free_bytes,
        "monitor_interval_seconds": 0.25,
        "monitor_limits": "sampled process RSS and filesystem free space; not instantaneous hard limits",
        "timings_are_gates": False,
    }
    write_json(out / "provenance.json", metadata)
    if metadata["free_bytes_before"] < args.min_free_bytes:
        raise ValueError("free space below experiment reserve; probe not started")
    started = time.monotonic()
    reason = None
    with (out / "stdout.txt").open("x") as stdout, (out / "stderr.txt").open("x") as stderr, \
            (out / "resource-samples.jsonl").open("x") as samples:
        process = subprocess.Popen(metadata["argv"], cwd=repo, stdout=stdout, stderr=stderr,
                                   env={**os.environ, "TMPDIR": str(scratch)}, start_new_session=True)
        try:
            while True:
                pid, status, usage = os.wait4(process.pid, os.WNOHANG)
                if pid:
                    process.returncode = os.waitstatus_to_exitcode(status)
                    break
                elapsed = time.monotonic() - started
                rss = rss_bytes(process.pid)
                free = shutil.disk_usage(scratch).free
                samples.write(json.dumps({"elapsed_seconds": elapsed, "rss_bytes": rss,
                                          "filesystem_free_bytes": free}) + "\n")
                samples.flush()
                if elapsed >= args.timeout_seconds:
                    reason = "experiment_timeout"
                elif rss is not None and rss > args.rss_limit_bytes:
                    reason = "experiment_rss_limit"
                elif free < args.min_free_bytes:
                    reason = "experiment_free_space_reserve"
                if reason is not None:
                    usage = stop(process)
                    break
                time.sleep(0.25)
        except BaseException as error:
            reason = f"monitor_error: {type(error).__name__}: {error}"
            usage = stop(process)
        ended = time.monotonic()
    source_after, patch_after = source_state(repo)
    (out / "source-after.patch").write_bytes(patch_after)
    source_changed = source_after != source_before
    binary_changed = digest(frozen) != binary_hash
    result = {
        "exit_code": process.returncode, "stop_reason": reason,
        "wall_seconds": ended - started,
        "wall_scope": "Popen start to observed wait4 completion; exit observation may lag up to one monitor interval",
        "user_cpu_seconds": usage.ru_utime, "system_cpu_seconds": usage.ru_stime,
        "peak_rss_kib": usage.ru_maxrss, "minor_faults": usage.ru_minflt,
        "major_faults": usage.ru_majflt, "input_blocks": usage.ru_inblock,
        "output_blocks": usage.ru_oublock,
        "voluntary_context_switches": usage.ru_nvcsw,
        "involuntary_context_switches": usage.ru_nivcsw,
        "resource_scope": "Linux wait4 for the single probe child; build excluded",
        "free_bytes_after": shutil.disk_usage(scratch).free,
        "source_after": source_after, "source_changed_during_run": source_changed,
        "frozen_binary_changed": binary_changed,
        "observations_after": {path: optional_text(path) for path in files[5:]},
        "process_completed": process.returncode == 0 and reason is None,
        "rss_budget_satisfied": usage.ru_maxrss * 1024 <= args.rss_limit_bytes,
        "provenance_stable": not source_changed and not binary_changed,
        "correctness_evidence": "existing probe assertions; inspect raw stdout and exit status",
    }
    write_json(out / "resource.json", result)
    print(json.dumps({"output": str(out), **{key: result[key] for key in (
        "exit_code", "stop_reason", "wall_seconds", "peak_rss_kib", "provenance_stable")}}))
    return 0 if (result["process_completed"] and result["provenance_stable"]
                 and result["rss_budget_satisfied"]) else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--workload", choices=("probe", "recovery-fixed", "recovery-growing",
                                               "phases-fixed", "phases-growing", "namespace"), required=True)
    parser.add_argument("--history", type=int)
    parser.add_argument("--namespace-files", type=int)
    parser.add_argument("--namespace-case", choices=("empty", "content-one", "content-new", "content-four",
                                                    "rename", "delete", "rename-conflict", "delete-conflict"))
    parser.add_argument("--payload-bytes", type=int, choices=(64, 128), default=128)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--build-command", required=True)
    parser.add_argument("--build-receipt", type=Path)
    parser.add_argument("--profile", required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--features", required=True)
    parser.add_argument("--timeout-seconds", type=float, default=60)
    parser.add_argument("--rss-limit-bytes", type=int, default=2 * 1024**3)
    parser.add_argument("--min-free-bytes", type=int, default=4 * 1024**3)
    args = parser.parse_args()
    try:
        return run(args)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Capture failed: {error}\n")


if __name__ == "__main__":
    raise SystemExit(main())
