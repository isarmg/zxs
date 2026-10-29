"""Pool-scoped accounting for strace -f -yy output; no timing conclusions."""

from __future__ import annotations

import re
from collections import Counter
from pathlib import Path


CALL = re.compile(r"^(?:(?P<pid>\d+)\s+)?(?P<name>\w+)\((?P<args>.*)\)\s+=\s+(?P<result>-?\d+)")
RESUMED = re.compile(r"^(?P<pid>\d+)\s+<\.\.\. (?P<name>\w+) resumed>(?P<tail>.*)$")
UNFINISHED = re.compile(r"^(?P<pid>\d+)\s+(?P<name>\w+)\((?P<args>.*)<unfinished \.\.\.>$")
FIRST_FD = re.compile(r"^\d+<(?P<path>[^>]+)>")
READS = frozenset(("read", "pread64", "readv", "preadv", "preadv2"))
WRITES = frozenset(("write", "pwrite64", "writev", "pwritev", "pwritev2"))


def parse_trace(trace: Path, pool: Path) -> dict:
    """Count successful pool FD calls; unresolved FD types stay explicitly unknown.

    FD annotations supplied by strace resolve cloned/duplicated descriptor paths.
    A disappeared temporary file cannot be classified after the run and is unknown.
    Returned bytes count successful syscall results, not arguments or disk sectors.
    """
    counts = Counter()
    pending = {}
    pool_path = str(pool.resolve())
    with trace.open(encoding="utf-8", errors="strict") as lines:
        for raw in lines:
            line = raw.rstrip("\n")
            unfinished = UNFINISHED.match(line)
            if unfinished:
                key = (unfinished["pid"], unfinished["name"])
                if key in pending:
                    raise ValueError(f"duplicate unfinished syscall: {key}")
                pending[key] = unfinished["args"]
                continue
            resumed = RESUMED.match(line)
            if resumed:
                key = (resumed["pid"], resumed["name"])
                args = pending.pop(key, None)
                if args is None:
                    counts["unmatched_resumed_calls"] += 1
                    continue
                line = f"{key[0]} {key[1]}({args}{resumed['tail']}"
            call = CALL.match(line)
            if call is None:
                if "(" in line and " = " in line:
                    counts["unparsed_call_lines"] += 1
                continue
            name, result = call["name"], int(call["result"])
            descriptor = FIRST_FD.match(call["args"])
            if descriptor is None:
                if name in READS | WRITES | {"fsync", "fdatasync"}:
                    counts["unresolved_fd_calls"] += 1
                continue
            path = descriptor["path"]
            if not (path == pool_path or path.startswith(pool_path + "/")):
                continue
            if path == pool_path + "/harness" or path.startswith(pool_path + "/harness/"):
                continue
            counts[f"{name}_calls"] += 1
            if result < 0:
                counts[f"{name}_failed_calls"] += 1
                continue
            if name in READS:
                counts["logical_read_bytes"] += result
            elif name in WRITES:
                counts["logical_write_bytes"] += result
            elif name in {"fsync", "fdatasync"}:
                target = Path(path.removesuffix(" (deleted)"))
                if target.is_dir():
                    kind = "directory"
                elif target.is_file():
                    kind = "file"
                else:
                    kind = "unknown"
                counts[f"{name}_{kind}_successful_calls"] += 1
    counts["unfinished_calls_at_end"] = len(pending)
    return {
        "scope": "successful syscall results on annotated file descriptors below this sample pool",
        "counter_semantics": "logical bytes; sync counts retain file/directory/unknown classification",
        "counters": dict(sorted(counts.items())),
        "complete": not pending and not any(
            counts[key] for key in ("unparsed_call_lines", "unmatched_resumed_calls", "unresolved_fd_calls")
        ),
    }
