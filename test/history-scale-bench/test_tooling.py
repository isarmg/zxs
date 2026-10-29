"""Evidence parser regressions; run by the dedicated GitHub validation carrier."""

import json
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
TOOLS = ROOT / "tools" / "history-scale-bench"
sys.path.insert(0, str(TOOLS))
from summarize import aggregate, operation_id, read_json, summarize_driver  # noqa: E402
from tracing import parse_trace  # noqa: E402


def valid_rows(operations=4):
    def telemetry(point):
        return {"phase": "telemetry", "point": point, **{key: 1 for key in ("vmrss_kib", "vmhwm_kib", "rchar", "wchar", "read_bytes", "write_bytes")}}

    def receipt(index):
        return {"operation_id": operation_id(index), "sequence": str(index + 1)}

    rows = [
        {"phase": "configuration", "stage": "build", "mode": "timing", "operations": operations, "payload_operations": operations - 1, "metadata_members": 1, "content_versions": 0, "verified": True},
        telemetry("process_start"),
        {"phase": "setup", "setup_receipts": 1, "setup_events": 1, "target_total_receipts": operations, "payload_operations": operations - 1, "result": receipt(0), "verified": True},
        telemetry("before_append"),
    ]
    for index in range(1, operations):
        rows.append({"phase": "append", "index": index, "operation_id": operation_id(index), "total_receipts": index + 1, "total_events": index + 1, "result": receipt(index), "latency_ns": (2, 100, 3)[index - 1], "verified": True})
    rows.append(telemetry("after_append"))
    for position, index in zip(("earliest", "middle", "recent"), (0, operations // 2, operations - 1)):
        for repetition in range(25):
            rows.append({"phase": "query", "position": position, "index": index, "operation_id": operation_id(index), "repetition": repetition, "cache_observation": "first_after_admission_or_append" if repetition == 0 else "repeated", "latency_ns": 10, "verified": True})
    pages = [
        {"phase": "events", "after_sequence": 0, "next_sequence": operations, "page_size": 257, "records": operations, "verified_events": operations, "latency_ns": 5, "verified": True},
        {"phase": "events", "after_sequence": operations, "next_sequence": operations, "page_size": 257, "records": 0, "verified_events": operations, "latency_ns": 5, "verified": True},
    ]
    rows.extend(pages)
    rows.append({"phase": "verify_receipts", "records": operations, "latency_ns": 5, "verified": True})
    for index in (0, operations // 2, operations - 1):
        rows.append({"phase": "retry", "index": index, "operation_id": operation_id(index), "latency_ns": 5, "verified": True})
    rows.extend(pages)
    rows.extend([
        telemetry("after_verification"),
        {"phase": "close", "latency_ns": 5, "verified": True},
        telemetry("after_close"),
        {"phase": "complete", "stage": "build", "operations": operations, "actual_total_receipts": operations, "actual_total_events": operations, "verified": True},
    ])
    return rows


def write_rows(path, rows):
    path.write_text("".join(json.dumps(row) + "\n" for row in rows), encoding="utf-8")


class EvidenceTests(unittest.TestCase):
    def test_duplicate_or_non_finite_input_is_rejected(self):
        for raw in ('{"phase":"append","phase":"complete"}', '{"latency_ns":NaN}'):
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                read_json(raw)

    def test_zero_exit_without_matching_completion_cannot_pass(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "result.jsonl"
            path.write_text('{"phase":"append","latency_ns":4,"verified":true}\n', encoding="utf-8")
            with self.assertRaises(ValueError):
                summarize_driver(path, "build", 256)
            path.write_text('{"phase":"complete","stage":"build","operations":255,"verified":true}\n', encoding="utf-8")
            with self.assertRaises(ValueError):
                summarize_driver(path, "build", 256)

    def test_latency_distribution_preserves_outliers_and_failures(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "result.jsonl"
            rows = valid_rows()
            write_rows(path, rows)
            result = summarize_driver(path, "build", 4)
            self.assertEqual(result["latencies"]["append"]["p99_ns"], 100)
            self.assertEqual(result["latencies"]["append"]["median_ns"], 3)
            rows[0]["verified"] = False
            write_rows(path, rows)
            with self.assertRaisesRegex(ValueError, "verification"):
                summarize_driver(path, "build", 4)

    def test_missing_or_duplicate_required_evidence_cannot_pass(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "result.jsonl"
            for phase in ("configuration", "append", "query", "events", "verify_receipts", "retry", "close", "telemetry"):
                rows = valid_rows()
                index = next(index for index, row in enumerate(rows) if row["phase"] == phase)
                for mutated in (rows[:index] + rows[index + 1:], rows[:index] + [rows[index]] + rows[index:]):
                    with self.subTest(phase=phase):
                        write_rows(path, mutated)
                        with self.assertRaises(ValueError):
                            summarize_driver(path, "build", 4)

    def test_query_positions_and_cache_observations_are_separate(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "result.jsonl"
            write_rows(path, valid_rows())
            result = summarize_driver(path, "build", 4)
            self.assertEqual(result["latencies"]["query.earliest.first_after_admission_or_append"]["count"], 1)
            self.assertEqual(result["latencies"]["query.recent.repeated"]["count"], 24)

    def test_trace_scopes_fd_paths_and_counts_real_results(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            pool = root / "pool"
            directory = pool / "disk" / "metadata"
            directory.mkdir(parents=True)
            product = directory / "journal"
            product.write_bytes(b"test")
            trace = root / "trace.txt"
            trace.write_text(
                f'10 write(3<{product}>, "12345678", 8) = 4\n'
                f'10 fsync(3<{product}>) = 0\n'
                f'10 fsync(4<{directory}>) = 0\n'
                f'10 fsync(5<{pool}/gone (deleted)>) = 0\n'
                f'10 read(3<{product}>,  <unfinished ...>\n'
                f'10 <... read resumed>"test", 4) = 4\n'
                f'10 write(6<{pool}/harness/receipts.jsonl>, "observer", 8) = 8\n'
                f'10 write(1<{root}/stdout>, "observer", 8) = 8\n'
                f'10 fdatasync(3<{product}>) = -1 EIO (Input/output error)\n',
                encoding="utf-8",
            )
            parsed = parse_trace(trace, pool)
            self.assertTrue(parsed["complete"])
            self.assertEqual(parsed["counters"]["logical_write_bytes"], 4)
            self.assertEqual(parsed["counters"]["logical_read_bytes"], 4)
            self.assertEqual(parsed["counters"]["fsync_directory_successful_calls"], 1)
            self.assertEqual(parsed["counters"]["fsync_file_successful_calls"], 1)
            self.assertEqual(parsed["counters"]["fsync_unknown_successful_calls"], 1)
            self.assertEqual(parsed["counters"]["fdatasync_failed_calls"], 1)

    def test_interrupted_trace_and_incomplete_sample_stay_incomplete(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            trace = root / "trace.txt"
            trace.write_text(f'10 fsync(3<{root}/pool/disk/journal> <unfinished ...>\n', encoding="utf-8")
            self.assertFalse(parse_trace(trace, root / "pool")["complete"])
            sample = root / "samples" / "example"
            sample.mkdir(parents=True)
            (sample / "manifest.json").write_text(json.dumps({"status": "incomplete", "mode": "timing", "stages": {}}), encoding="utf-8")
            result = aggregate(root)
            self.assertFalse(result["all_requested_samples_passed"])
            self.assertEqual(result["status_counts"], {"incomplete": 1})


if __name__ == "__main__":
    unittest.main()
