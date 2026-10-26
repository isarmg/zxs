#!/usr/bin/env python3
"""Compare identical fresh-prepare and export operations in two engine test binaries.

Each fixture brackets the complete operation, excluding fixture creation and
post-operation integrity checks. Untraced samples report /proc/self/io bytes,
elapsed time, current RSS and lifetime process peak RSS (including fixture setup).
strace additionally checks exact successful payload read and write bytes, retaining
raw traces. --no-trace is for restricted hosts and never claims exact syscall data.
"""
import argparse
import collections
import json
from pathlib import Path
import re
import statistics
import subprocess

FIXTURES = {
    "fresh_prepare": "engine::payload_measurement::fresh_prepare_operation",
    "export": "engine::payload_measurement::export_operation",
}
EXPECTED_READ_PASSES = {
    "baseline": {"fresh_prepare": 6, "export": 2},
    "candidate": {"fresh_prepare": 3, "export": 1},
}
RESULT = "ZXS_PAYLOAD_MEASURE_RESULT "


def completed_lines(lines):
    """Join strace's per-thread unfinished/resumed syscall pairs."""
    pending = {}
    for line in lines:
        pid = line.split(maxsplit=1)[0]
        if "<unfinished ...>" in line:
            if pid in pending:
                raise ValueError(f"nested unfinished syscall for {pid}")
            pending[pid] = line.replace("<unfinished ...>", "")
        elif "<... " in line and " resumed>" in line:
            suffix = line.split(" resumed>", 1)[1]
            yield pending.pop(pid) + suffix
        else:
            yield line
    if pending:
        raise ValueError(f"unfinished syscall records: {pending}")


def body_descriptor(line):
    match = re.search(r"\((\d+)<([^>]+)>", line)
    if not match:
        return False
    path = match.group(2)
    return (path.endswith(".full")
            or path.endswith("/content/payload-measurement-body.bin")
            or ("/file-containers/" in path and "/versions/" in path and path.endswith(".bin"))
            or "/.zxs-full-" in path)


def syscall_sample(lines, operation):
    active = False
    finished = False
    reads = writes = 0
    calls = collections.Counter()
    for line in completed_lines(lines):
        if f"ZXS_PAYLOAD_MEASURE_BEGIN {operation}" in line:
            if active or finished:
                raise ValueError("duplicate operation start marker")
            active = True
            continue
        if f"ZXS_PAYLOAD_MEASURE_END {operation}" in line:
            if not active:
                raise ValueError("end marker without start")
            active = False
            finished = True
            continue
        if not active:
            continue
        match = re.search(r"\b(read|pread64|write|pwrite64|fsync|fdatasync|renameat2)\(", line)
        if not match:
            continue
        name = match.group(1)
        calls[name] += 1
        result = re.search(r" = (\d+)(?:\s|$)", line)
        if result and body_descriptor(line):
            count = int(result.group(1))
            if name in ("read", "pread64"):
                reads += count
            elif name in ("write", "pwrite64"):
                writes += count
    if not finished or active:
        raise ValueError("missing complete operation marker pair")
    return {"payload_read_bytes": reads, "payload_write_bytes": writes,
            "syscalls": dict(sorted(calls.items()))}


def run(binary, output, label, operation, suffix, trace=False):
    stem = output / f"{label}-{operation}-{suffix}"
    command = [str(binary), FIXTURES[operation], "--exact", "--ignored",
               "--nocapture", "--test-threads=1"]
    if trace:
        command = ["strace", "-f", "-yy", "-s", "256", "-e",
                   "trace=read,pread64,write,pwrite64,fsync,fdatasync,renameat2",
                   "-o", str(stem.with_suffix(".strace")), *command]
    result = subprocess.run(command, text=True, capture_output=True)
    log = result.stdout + result.stderr
    stem.with_suffix(".log").write_text(log)
    if result.returncode != 0 or "1 passed; 0 failed" not in result.stdout:
        raise RuntimeError(f"{label}/{operation}: fixture or trace failed; see {stem}.log")
    samples = [json.loads(line.split(RESULT, 1)[1])
               for line in log.splitlines() if RESULT in line]
    if len(samples) != 1 or samples[0]["operation"] != operation:
        raise ValueError(f"expected one {operation} sample: {samples}")
    sample = samples[0]
    if trace:
        sample["trace"] = syscall_sample(stem.with_suffix(".strace").read_text().splitlines(), operation)
        expected_reads = EXPECTED_READ_PASSES[label][operation] * sample["payload_bytes"]
        expected_writes = (3 if operation == "fresh_prepare" else 1) * sample["payload_bytes"]
        if sample["trace"]["payload_read_bytes"] != expected_reads:
            raise AssertionError(f"{label}/{operation}: expected {expected_reads} body read bytes, got {sample['trace']}")
        if sample["trace"]["payload_write_bytes"] != expected_writes:
            raise AssertionError(f"{label}/{operation}: expected {expected_writes} body write bytes, got {sample['trace']}")
    return sample


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline", type=Path)
    parser.add_argument("candidate", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--rounds", type=int, default=3)
    parser.add_argument("--no-trace", action="store_true")
    args = parser.parse_args()
    if args.rounds < 1:
        parser.error("--rounds must be positive")
    args.output.mkdir(parents=True, exist_ok=True)
    binaries = {"baseline": args.baseline.resolve(), "candidate": args.candidate.resolve()}
    results = {label: {operation: {"samples": []} for operation in FIXTURES} for label in binaries}
    for round_number in range(args.rounds):
        order = list(binaries) if round_number % 2 == 0 else list(reversed(binaries))
        for operation in FIXTURES:
            for label in order:
                sample = run(binaries[label], args.output, label, operation, f"round-{round_number}")
                results[label][operation]["samples"].append(sample)
    for label, operations in results.items():
        for operation, result in operations.items():
            result["median"] = {key: statistics.median(sample[key] for sample in result["samples"])
                                for key in result["samples"][0] if key != "operation"}
            if not args.no_trace:
                result["traced_sample"] = run(binaries[label], args.output, label, operation, "trace", trace=True)
    summary = {"scope": "complete prepare_payload with three routes; complete export_version; setup and post-checks excluded",
               "memory_scope": "current RSS at operation boundaries; process peak includes fixture setup",
               "io_scope": "rchar/wchar include metadata and sampling overhead; exact body bytes only in trace",
               "trace_status": "not requested" if args.no_trace else "passed exact body byte assertions",
               "results": results}
    (args.output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
