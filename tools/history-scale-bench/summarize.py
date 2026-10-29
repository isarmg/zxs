"""Validate driver evidence and summarize paired history benchmark samples."""

from __future__ import annotations

import json
import math
import statistics
from collections import defaultdict
from pathlib import Path


def unique_object(pairs: list) -> dict:
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON field: {key}")
        result[key] = value
    return result


def reject_constant(value: str) -> None:
    raise ValueError(f"non-finite JSON number: {value}")


def read_json(line: str) -> object:
    return json.loads(line, object_pairs_hook=unique_object, parse_constant=reject_constant)


def distribution(values: list[int]) -> dict:
    ordered = sorted(values)
    if not ordered:
        return {"count": 0}

    def percentile(fraction: float) -> int:
        return ordered[max(0, math.ceil(len(ordered) * fraction) - 1)]

    return {
        "count": len(ordered),
        "min_ns": ordered[0],
        "median_ns": statistics.median(ordered),
        "p95_ns": percentile(0.95),
        "p99_ns": percentile(0.99),
        "max_ns": ordered[-1],
        "sum_ns": sum(ordered),
    }


def operation_id(index: int) -> str:
    bits = ((index + 1) * 0x9E3779B97F4A7C15D6E8FEB86659FD93) % (1 << 128)
    return f"op_{bits:032x}"


class ResultReader:
    def __init__(self, lines):
        self.lines = iter(lines)
        self.rows = 0
        self.latencies = defaultdict(list)
        self.telemetry = []

    def take(self, phase: str, **expected) -> dict:
        try:
            row = read_json(next(self.lines))
        except StopIteration as error:
            raise ValueError(f"missing {phase} row/completion marker") from error
        self.rows += 1
        if not isinstance(row, dict) or row.get("phase") != phase:
            raise ValueError(f"expected {phase} at row {self.rows}")
        if phase != "telemetry" and row.get("verified") is not True:
            raise ValueError(f"missing successful verification at row {self.rows}")
        for key, value in expected.items():
            if row.get(key) != value or (type(value) is int and type(row.get(key)) is not int):
                raise ValueError(f"{phase}.{key} does not match expected {value!r}")
        timed = {"append", "query", "events", "verify_receipts", "retry", "close", "reopen", "recovery"}
        if phase in timed and "latency_ns" not in row:
            raise ValueError(f"missing latency for {phase}")
        if "latency_ns" in row:
            latency = row["latency_ns"]
            if type(latency) is not int or latency < 0:
                raise ValueError(f"invalid latency at row {self.rows}")
            group = phase
            if phase == "query":
                group = f"query.{row['position']}.{row['cache_observation']}"
            self.latencies[group].append(latency)
        return row

    def telemetry_at(self, point: str) -> None:
        row = self.take("telemetry", point=point)
        for counter in ("vmrss_kib", "vmhwm_kib", "rchar", "wchar", "read_bytes", "write_bytes"):
            if type(row.get(counter)) is not int or row[counter] < 0:
                raise ValueError(f"invalid telemetry counter {counter}")
        self.telemetry.append(row)


def verify_result(row: dict, index: int) -> None:
    result = row.get("result")
    if not isinstance(result, dict) or result.get("operation_id") != operation_id(index):
        raise ValueError("receipt identity mismatch")
    if result.get("sequence") != str(index + 1):
        raise ValueError("receipt sequence mismatch")


def verify_pages(reader: ResultReader, operations: int) -> None:
    cursor = 0
    while True:
        row = reader.take("events", after_sequence=cursor, page_size=257)
        count = row.get("records")
        if type(count) is not int or count < 0 or count > 257:
            raise ValueError("invalid event page length")
        cursor += count
        if cursor > operations or row.get("next_sequence") != cursor or row.get("verified_events") != cursor:
            raise ValueError("event page cursor/count mismatch")
        if not count:
            if cursor != operations:
                raise ValueError("missing events before terminal page")
            return


def summarize_driver(path: Path, stage: str, operations: int, mode: str = "timing", baseline: bool | None = None) -> dict:
    """Require the complete, ordered driver protocol; a green marker alone fails."""
    with path.open(encoding="utf-8") as lines:
        reader = ResultReader(lines)
        configuration = reader.take("configuration", stage=stage, operations=operations, mode=mode,
                                    payload_operations=operations - 1, metadata_members=1, content_versions=0)
        if baseline is not None and configuration.get("history_scale_baseline") is not baseline:
            raise ValueError("driver build feature does not match revision")
        reader.telemetry_at("process_start")
        if stage == "build":
            setup = reader.take("setup", setup_receipts=1, setup_events=1, target_total_receipts=operations,
                                payload_operations=operations - 1)
            verify_result(setup, 0)
            reader.telemetry_at("before_append")
            for index in range(1, operations):
                row = reader.take("append", index=index, operation_id=operation_id(index),
                                  total_receipts=index + 1, total_events=index + 1)
                verify_result(row, index)
            reader.telemetry_at("after_append")
        else:
            reader.telemetry_at("after_admission_before_oracle")
            reader.take(stage)
        selected = list(dict.fromkeys((0, operations // 2, operations - 1)))
        for offset, index in enumerate(selected):
            position = "earliest" if offset == 0 else "recent" if offset == len(selected) - 1 else "middle"
            for repetition in range(25):
                reader.take("query", position=position, index=index, operation_id=operation_id(index),
                            repetition=repetition, cache_observation="first_after_admission_or_append" if repetition == 0 else "repeated")
        verify_pages(reader, operations)
        reader.take("verify_receipts", records=operations)
        for index in selected:
            reader.take("retry", index=index, operation_id=operation_id(index))
        verify_pages(reader, operations)
        reader.telemetry_at("after_verification")
        reader.take("close")
        reader.telemetry_at("after_close")
        complete = reader.take("complete", stage=stage, operations=operations,
                               actual_total_receipts=operations, actual_total_events=operations)
        if next(reader.lines, None) is not None:
            raise ValueError("unexpected row after completion")
    return {
        "rows": reader.rows, "completion": complete, "configuration": configuration,
        "telemetry": reader.telemetry,
        "latencies": {phase: distribution(values) for phase, values in sorted(reader.latencies.items())},
    }


def aggregate(output: Path) -> dict:
    manifests = sorted(output.glob("samples/*/manifest.json"))
    samples = [read_json(path.read_text(encoding="utf-8")) for path in manifests]
    requested_path = output / "requested.json"
    requested = read_json(requested_path.read_text(encoding="utf-8")) if requested_path.is_file() else {"cohort_complete": False, "samples": []}
    expected = {row["sample"]: row for row in requested["samples"]}
    observed = {path.parent.name: sample for path, sample in zip(manifests, samples)}
    matrix_matches = bool(expected) and len(expected) == len(requested["samples"]) and expected.keys() == observed.keys()
    if matrix_matches:
        matrix_matches = all(
            all(observed[name].get(key) == row[key] for key in ("label", "operations", "repeat", "mode"))
            for name, row in expected.items()
        )
    counts = defaultdict(int)
    groups = defaultdict(list)
    for sample in samples:
        counts[sample["status"]] += 1
        if sample["mode"] != "timing":
            continue
        for stage, result in sample["stages"].items():
            if result["status"] != "passed":
                continue
            for phase, latency in result["driver"]["latencies"].items():
                key = (sample["label"], sample["operations"], stage, phase)
                groups[key].append(latency)
    variation = []
    for (label, operations, stage, phase), runs in sorted(groups.items()):
        medians = [run["median_ns"] for run in runs]
        variation.append({
            "label": label, "operations": operations, "stage": stage, "phase": phase,
            "runs": len(runs), "run_median_ns": medians,
            "min_run_median_ns": min(medians), "max_run_median_ns": max(medians),
        })
    return {
        "status_counts": dict(sorted(counts.items())),
        "cohort_complete": requested.get("cohort_complete") is True,
        "requested_matrix_matches": matrix_matches,
        "missing_requested_samples": sorted(expected.keys() - observed.keys()),
        "unexpected_samples": sorted(observed.keys() - expected.keys()),
        "all_requested_samples_passed": requested.get("cohort_complete") is True and matrix_matches and all(
            sample["status"] == "passed" and set(sample["stages"]) == {"build", "reopen", "recovery"}
            and all(stage["status"] == "passed" for stage in sample["stages"].values())
            for sample in samples
        ),
        "timing_variation": variation,
        "samples": samples,
        "limits": [
            "Warm filesystem/page cache; fresh process reopening is not a physical cold-cache test.",
            "Pools use logical member directories on one runner filesystem, not independent physical disks.",
            "RSS is resident memory, not allocations. /proc I/O bytes and host device statistics have distinct scopes.",
            "Only untraced timing samples contribute to timing variation; counts samples are not latency evidence.",
        ],
    }
