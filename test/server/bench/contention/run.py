#!/usr/bin/env python3
"""Bounded paired measurements of the real Backend mutex in release test builds."""
import argparse
import json
import os
from pathlib import Path
import re
import resource
import shutil
import signal
import subprocess
import tempfile
import time
import traceback

GIB = 1024 ** 3
SCALES = (500, 1500, 3000)
# Capacity-planning allowances, not proven peak usage. See README: old retained
# pools plus headroom for the 64 MiB body; the independent 8 GiB reserve remains.
POOL_PREFLIGHT_GIB = {500: 3, 1500: 7, 3000: 16}
PREFIX = "engine_backend::contention_measurements::contention_"


class Incomplete(RuntimeError):
    """The declared observation/resource coverage was not established."""


def write_json(path, value):
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(json.dumps(value, indent=2) + "\n")
    temporary.replace(path)


def limits():
    resource.setrlimit(resource.RLIMIT_AS, (8 * GIB, 8 * GIB))


def stop(child):
    # This session contains only the owned measurement and any descendants.
    try:
        os.killpg(child.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        child.wait(timeout=10)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        child.wait()


def execute(command, prefix, pool, deadline):
    record = dict(command=command, status="running", started_unix_ns=time.time_ns(),
                  timeout_seconds=900, minimum_free_bytes=8 * GIB,
                  address_space_limit_bytes=8 * GIB)
    status_path = prefix.with_name(prefix.name + "-process.json")
    write_json(status_path, record)
    environment = dict(os.environ, TMPDIR=str(pool))
    child = None
    try:
        if time.time() >= deadline:
            raise Incomplete("global_time_budget")
        with prefix.with_suffix(".stdout").open("wb") as stdout, prefix.with_suffix(".stderr").open("wb") as stderr:
            child = subprocess.Popen(command, stdout=stdout, stderr=stderr, env=environment,
                                     start_new_session=True, preexec_fn=limits)
            local_deadline = time.monotonic() + 900
            record["pid"] = child.pid
            write_json(status_path, record)
            while child.poll() is None:
                if time.time() >= deadline:
                    raise Incomplete("global_time_budget")
                if time.monotonic() >= local_deadline:
                    raise Incomplete("process_time_budget")
                available = shutil.disk_usage(pool).free
                if available < 8 * GIB:
                    record["observed_free_bytes"] = available
                    raise Incomplete("runner_free_space_budget")
                time.sleep(0.5)
            record["returncode"] = child.wait()
            if record["returncode"] != 0:
                raise subprocess.CalledProcessError(record["returncode"], command)
        record["status"] = "completed"
    except Incomplete as error:
        record.update(status="incomplete", reason=str(error))
        raise
    except (OSError, subprocess.SubprocessError) as error:
        record.update(status="failed", error_type=type(error).__name__, error=str(error))
        raise
    finally:
        if child is not None and child.poll() is None:
            stop(child)
        record["finished_unix_ns"] = time.time_ns()
        write_json(status_path, record)


def observations(stdout, files):
    assert "1 passed; 0 failed; 0 ignored" in stdout
    rows = [json.loads(line.removeprefix("CONTENTION_EVIDENCE "))
            for line in stdout.splitlines() if line.startswith("CONTENTION_EVIDENCE ")]
    assert len(rows) == 1, "expected exactly one measured fixture body"
    record = rows[0]
    assert record["files"] == files
    assert record["mutex_owner_hold_time_measured"] is False
    if not record["probe_complete"]:
        raise Incomplete("observer_attempt_budget")
    begin, end = record["writer"]["start_ns"], record["writer"]["end_ns"]
    assert 0 <= begin < end
    assert record["read_kinds"] == ["directory", "file_history", "operation"]
    overlap = {}
    read_bounds = {}
    for kind, calls in zip(record["read_kinds"], record["reads"], strict=True):
        assert len(calls) == 32
        assert all(0 <= row["start_ns"] <= row["end_ns"] for row in calls)
        overlap[kind] = sum(row["start_ns"] < end and row["end_ns"] > begin for row in calls)
        read_bounds[kind] = [row["end_ns"] - row["start_ns"] for row in calls]
    samples = record["mutex_acquisition_elapsed"]
    assert all(0 <= row["start_ns"] <= row["acquired_ns"] for row in samples)
    acquired = sorted(row["acquired_ns"] for row in samples if begin < row["acquired_ns"] < end)
    if not acquired:
        raise Incomplete("no_observer_acquisition_inside_writer")
    if not all(overlap.values()):
        raise Incomplete(f"missing_read_write_overlap: {overlap}")
    boundaries = [begin, *acquired, end]
    # At each acquired timestamp the observer owns this actual mutex, so no
    # continuous writer-held section can cross it. This is only an upper bound.
    record.update(reads_overlapping_writer=overlap,
                  observer_acquisitions_inside_writer=len(acquired),
                  conservative_bounds=dict(
                      writer_total_hold_upper_bound_ns=end - begin,
                      writer_continuous_hold_gap_upper_bound_ns=max(
                          right - left for left, right in zip(boundaries, boundaries[1:])),
                      reader_total_hold_upper_bounds_ns=read_bounds,
                      exact_owner_hold_time_measured=False,
                      scope="observed release-test calls; includes scheduler/observer effects, not a production SLA"))
    return record


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--baseline-sha", required=True)
    parser.add_argument("--candidate-sha", required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--pool-parent", type=Path, required=True)
    parser.add_argument("--deadline-unix", type=int, required=True)
    parser.add_argument("--sizes", type=int, nargs="+", default=list(SCALES))
    args = parser.parse_args()
    for sha in (args.baseline_sha, args.candidate_sha):
        if re.fullmatch(r"[0-9a-f]{40}", sha) is None:
            parser.error("source SHAs must be full Git object IDs")
    if not args.sizes or any(size not in SCALES for size in args.sizes):
        parser.error("sizes must be drawn from 500, 1500, 3000")
    binaries = {"baseline": args.baseline.resolve(), "candidate": args.candidate.resolve()}
    args.out.mkdir(parents=True, exist_ok=False)
    args.pool_parent.mkdir(parents=True, exist_ok=True)
    summary = dict(status="incomplete", runs=[], deadline_unix=args.deadline_unix,
                   baseline_sha=args.baseline_sha, candidate_sha=args.candidate_sha,
                   configuration="release test binaries with existing development features",
                   scope="real Backend intervals/mutex acquisition and conservative occupancy bounds; not adoption")
    summary_path = args.out / "summary.json"
    write_json(summary_path, summary)
    active = None
    try:
        for variant, binary in binaries.items():
            if time.time() >= args.deadline_unix:
                raise Incomplete("global_time_budget")
            listed = subprocess.run([str(binary), "--list"], text=True, capture_output=True,
                                    check=True, timeout=60)
            (args.out / f"{variant}-inventory.txt").write_text(listed.stdout)
            for files in args.sizes:
                assert f"{PREFIX}{files}: test" in listed.stdout
        for files in args.sizes:
            for trial in range(3):
                order = ("baseline", "candidate") if trial % 2 == 0 else ("candidate", "baseline")
                for variant in order:
                    tag = f"{files}-{trial}-{variant}"
                    active = dict(case=tag, variant=variant, trial=trial, files=files, status="running")
                    summary["runs"].append(active)
                    write_json(summary_path, summary)
                    if time.time() >= args.deadline_unix:
                        raise Incomplete("global_time_budget")
                    available = shutil.disk_usage(args.pool_parent).free
                    required = (POOL_PREFLIGHT_GIB[files] + 8) * GIB
                    active.update(required_free_bytes=required, observed_free_bytes=available,
                                  pool_planning_allowance_bytes=POOL_PREFLIGHT_GIB[files] * GIB,
                                  reserved_free_bytes=8 * GIB)
                    if available < required:
                        raise Incomplete("pool_space_preflight")
                    command = [str(binaries[variant]), f"{PREFIX}{files}", "--exact", "--ignored",
                               "--nocapture", "--test-threads=1"]
                    prefix = args.out / tag
                    with tempfile.TemporaryDirectory(prefix="owned-contention-", dir=args.pool_parent) as pool:
                        execute(command, prefix, pool, args.deadline_unix)
                    record = observations(prefix.with_suffix(".stdout").read_text(), files)
                    record.update(variant=variant, trial=trial)
                    write_json(args.out / f"{tag}-result.json", record)
                    active["status"] = "measured"
                    write_json(summary_path, summary)
                    print(json.dumps({"case": tag, "status": "measured"}), flush=True)
        summary["status"] = "completed_measurements"
        return 0
    except Incomplete as error:
        summary.update(status="incomplete", reason=str(error))
        if active is not None:
            active.update(status="incomplete", reason=str(error))
        return 2
    except subprocess.TimeoutExpired as error:
        summary.update(status="incomplete", reason="inventory_time_budget", error=str(error))
        for stream in ("stdout", "stderr"):
            partial = getattr(error, stream)
            if partial is not None:
                (args.out / f"{variant}-inventory.{stream}").write_bytes(
                    partial.encode() if isinstance(partial, str) else partial)
        return 2
    except (OSError, ValueError, KeyError, TypeError, RuntimeError, AssertionError, subprocess.SubprocessError) as error:
        summary.update(status="failed", error_type=type(error).__name__, error=str(error))
        if active is not None:
            active.update(status="failed", error_type=type(error).__name__, error=str(error))
        (args.out / "runner-error.txt").write_text(traceback.format_exc())
        return 1
    finally:
        write_json(summary_path, summary)


if __name__ == "__main__":
    raise SystemExit(main())
