#!/usr/bin/env python3
"""Compare unchanged journal recovery fixture in baseline and candidate test binaries.

Build both with `cargo test -p storage-engine --lib --no-run`, then pass their
executable paths. Requires Linux strace/ptrace permission; never fabricates counts
when tracing is unavailable. Raw traces accompany the JSON summary.
"""
import argparse
import collections
import json
from pathlib import Path
import re
import subprocess
import time

FIXTURE = "journal::scanner_tests::typed_phases_survive_normal_publication_and_recovery"


def measure(binary, output, label):
    trace = output / f"{label}.strace"
    started = time.perf_counter_ns()
    result = subprocess.run([
        "strace", "-f", "-yy", "-e",
        "trace=openat,openat2,newfstatat,fstat,statx,read,write,fsync,fdatasync,close",
        "-o", str(trace), str(binary), FIXTURE, "--exact", "--nocapture",
    ], text=True, capture_output=True)
    elapsed = time.perf_counter_ns() - started
    (output / f"{label}.log").write_text(result.stdout + result.stderr)
    if result.returncode != 0 or "1 passed; 0 failed" not in result.stdout:
        raise SystemExit(f"{label}: tracing or fixture failed; see {output / (label + '.log')}")
    return {"fixture": FIXTURE, "elapsed_ns_with_tracing": elapsed, "syscalls": syscall_counts(trace.read_text().splitlines())}


def syscall_counts(lines):
    counts = collections.Counter()
    for line in lines:
        match = re.search(r"\b(openat2?|newfstatat|fstat|statx|read|write|fsync|fdatasync|close)\(", line)
        if match and "application-log" in line:
            kind = "journal" if "journal.bin" in line else "directory"
            counts[f"{kind}.{match.group(1)}"] += 1
    return dict(sorted(counts.items()))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline", type=Path)
    parser.add_argument("candidate", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    results = {label: measure(binary.resolve(), args.output, label)
               for label, binary in [("baseline", args.baseline), ("candidate", args.candidate)]}
    (args.output / "summary.json").write_text(json.dumps(results, indent=2) + "\n")
    print(json.dumps(results, indent=2))


if __name__ == "__main__":
    main()
