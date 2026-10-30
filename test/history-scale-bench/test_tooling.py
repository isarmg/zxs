"""Evidence parser regressions; run by the dedicated GitHub validation carrier."""

import errno
import copy
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
TOOLS = ROOT / "tools" / "history-scale-bench"
sys.path.insert(0, str(TOOLS))
from summarize import aggregate, operation_id, read_json, summarize_driver  # noqa: E402
from tracing import parse_trace  # noqa: E402
from run import arguments, final_budget_reason, process_snapshot, validate_build_identity  # noqa: E402


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
        rows.append({"phase": "append", "index": index, "operation_id": operation_id(index), "total_receipts": index + 1, "total_events": index + 1, "result": receipt(index), "latency_ns": (2, 100, 3)[(index - 1) % 3], "verified": True})
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


def separated_rows():
    rows = valid_rows(8)
    rows[0].update(query_protocol="separate-outcome-health", fixture_mode="fixed-body", setup_receipts=2,
                   setup_events=2, content_versions=1, content_bytes=4096, fill_byte=90, payload_operations=6,
                   history_scale_baseline=False, history_scale_current=True)
    rows[2].update(index=0, payload_operations=6)
    seed_result = {"operation_id": operation_id(1), "sequence": "2"}
    rows.insert(3, {"phase": "setup", "index": 1, "setup_receipts": 2, "setup_events": 2,
                    "seed_path": "fixture.bin", "seed_file_id": "file", "seed_version_id": "version",
                    "content_bytes": 4096, "commit_request": {"operation_id": operation_id(1)},
                    "result": seed_result, "verified": True})
    rows = [row for row in rows if not (row["phase"] == "append" and row["index"] == 1)]
    for row in rows:
        if row["phase"] == "query":
            row["query_kind"] = "permanent_outcome"
    health = []
    for position, index in (("content_fixture", 1), ("middle", 4), ("recent", 7)):
        for repetition in range(25):
            health.append({"phase": "health_query", "query_kind": "current_health", "position": position,
                           "index": index, "operation_id": operation_id(index), "repetition": repetition,
                           "scope_versions": 1 if index == 1 else 0, "health_observed": index == 1,
                           "health_state": "HEALTHY" if index == 1 else None, "observation_contract": "native_observe",
                           "cache_observation": "first_after_outcome_queries" if repetition == 0 else "repeated",
                           "latency_ns": 11, "verified": True})
    first_events = next(index for index, row in enumerate(rows) if row["phase"] == "events")
    rows[first_events:first_events] = health
    content = {"phase": "verify_content", "path": "fixture.bin", "file_id": "file", "version_id": "version",
               "content_bytes": 4096, "fill_byte": 90, "history_versions": 1, "history_records_verified": True,
               "latency_ns": 12, "verified": True}
    before_retries = next(index for index, row in enumerate(rows) if row["phase"] == "retry")
    rows.insert(before_retries, {**content, "after_retry": False})
    after_retries = max(index for index, row in enumerate(rows) if row["phase"] == "retry") + 1
    rows[after_retries:after_retries] = [
        {"phase": "retry_content", "index": 1, "operation_id": operation_id(1), "result": seed_result,
         "no_new_publication": True, "latency_ns": 13, "verified": True},
        {**content, "after_retry": True},
    ]
    return rows


class EvidenceTests(unittest.TestCase):
    def test_new_fixed_content_protocol_separates_outcome_and_live_health(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "driver.jsonl"
            write_rows(path, separated_rows())
            result = summarize_driver(path, "build", 8, baseline=False, required_protocol="separate-outcome-health")
            self.assertEqual(result["configuration"]["payload_operations"], 6)
            self.assertEqual(result["latencies"]["query.permanent_outcome.earliest.repeated"]["count"], 24)
            self.assertEqual(result["latencies"]["health_query.content_fixture.first_after_outcome_queries"]["count"], 1)
            self.assertEqual(result["latencies"]["verify_content"]["count"], 2)

    def test_new_protocol_requires_health_content_retry_and_original_identity(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "driver.jsonl"
            for phase in ("health_query", "verify_content", "retry_content"):
                rows = separated_rows()
                index = next(index for index, row in enumerate(rows) if row["phase"] == phase)
                rows.pop(index)
                write_rows(path, rows)
                with self.subTest(phase=phase), self.assertRaises(ValueError):
                    summarize_driver(path, "build", 8, required_protocol="separate-outcome-health")
            for phase, key, value in (("health_query", "health_observed", 1), ("retry_content", "no_new_publication", False),
                                      ("verify_content", "version_id", "changed"), ("query", "query_kind", "current_health")):
                rows = copy.deepcopy(separated_rows())
                next(row for row in rows if row["phase"] == phase)[key] = value
                write_rows(path, rows)
                with self.subTest(phase=phase, key=key), self.assertRaises(ValueError):
                    summarize_driver(path, "build", 8, required_protocol="separate-outcome-health")

    def test_unobserved_health_requires_explicit_null_state(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "driver.jsonl"
            rows = separated_rows()
            unobserved = next(row for row in rows if row["phase"] == "health_query" and row["health_observed"] is False)
            unobserved.pop("health_state")
            write_rows(path, rows)
            with self.assertRaisesRegex(ValueError, "health_state"):
                summarize_driver(path, "build", 8, required_protocol="separate-outcome-health")

    def test_reuse_validates_original_build_identity_separately_from_observer(self):
        identity = {"GITHUB_SHA": "a" * 40, "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "1"}
        with patch.dict(os.environ, {"GITHUB_SHA": "b" * 40, "GITHUB_RUN_ID": "456", "GITHUB_RUN_ATTEMPT": "2"}):
            self.assertIsNone(validate_build_identity(identity, "123", "a" * 40))
        for run_id, carrier in (("456", "a" * 40), ("123", "b" * 40)):
            with self.subTest(run_id=run_id, carrier=carrier), self.assertRaisesRegex(ValueError, "explicitly selected"):
                validate_build_identity(identity, run_id, carrier)
        self.assertEqual(identity["GITHUB_RUN_ATTEMPT"], "1")

    def test_legacy_archive_can_be_read_but_cannot_satisfy_new_run_protocol(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "legacy.jsonl"
            write_rows(path, valid_rows())
            self.assertEqual(summarize_driver(path, "build", 4)["configuration"]["content_versions"], 0)
            with self.assertRaisesRegex(ValueError, "required query protocol"):
                summarize_driver(path, "build", 4, required_protocol="separate-outcome-health")

    def test_runner_profile_uses_plan_and_rejects_arbitrary_sizes_before_execution(self):
        plan = {"schema_version": 1, "baseline_sha": "e2f33d9882cf7263412d9370b6a703e0b216357d",
                "reviewed_candidate_sha": "c8ce99de07f2d0e673dcc71f9278f6ac570b3633", "frozen_candidate_sha": None,
                "ten_thousand_review": None,
                "profiles": ["small-selftest", "boundaries", "1000", "10000"]}
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            plan_path = root / "plan.json"
            plan_path.write_text(json.dumps(plan), encoding="utf-8")
            placeholder = root / "placeholder"
            placeholder.write_text("fixture", encoding="utf-8")
            command = ["run.py", "--plan", str(plan_path), "--profile", "small-selftest", "--output-root", str(root / "results"),
                       "--build-run-id", "123", "--build-carrier-sha", "a" * 40]
            for label in ("baseline", "candidate"):
                command.extend([f"--{label}-binary", str(placeholder), f"--{label}-source", str(root), f"--{label}-provenance", str(placeholder)])
            with patch.object(sys, "argv", command):
                parsed = arguments()
            self.assertEqual(parsed.sizes, [8])
            self.assertFalse(parsed.scale_authorized)
            self.assertEqual(parsed.phase_seconds, 60)
            for option, value in (("--build-run-id", "0"), ("--build-run-id", "123x"), ("--build-carrier-sha", "main")):
                malformed = command.copy()
                malformed[malformed.index(option) + 1] = value
                with self.subTest(option=option, value=value), patch.object(sys, "argv", malformed), self.assertRaises(SystemExit):
                    arguments()
            with patch.object(sys, "argv", command + ["--sizes", "1000"]), self.assertRaises(SystemExit):
                arguments()
            command[command.index("small-selftest")] = "1000"
            with patch.object(sys, "argv", command), self.assertRaisesRegex(ValueError, "frozen"):
                arguments()

    def test_io_permission_race_requires_proven_exit_and_keeps_diagnostic(self):
        live = "State:\tR (running)\nVmRSS:\t17 kB\nVmHWM:\t19 kB\nVmSize:\t23 kB\n"
        for state in ("Z", "X", "disappeared"):
            with self.subTest(state=state):
                status_reads = []

                def read(path, **_kwargs):
                    if str(path).endswith("/io"):
                        raise PermissionError(errno.EACCES, "permission denied", str(path))
                    status_reads.append(path)
                    if len(status_reads) == 1:
                        return live
                    if state == "disappeared":
                        raise FileNotFoundError(errno.ENOENT, "exited", str(path))
                    return f"State:\t{state} (exited)\n"

                with patch.object(Path, "read_text", read):
                    snapshot = process_snapshot(5697)
                self.assertFalse(snapshot["valid"])
                self.assertEqual(snapshot["memory"], {})
                self.assertIsNone(snapshot["io"])
                self.assertEqual(snapshot["io_error"], {"pid": 5697, "path": "/proc/5697/io", "errno": errno.EACCES, "state": state})

    def test_live_io_permission_and_unexpected_errors_still_propagate(self):
        live = "State:\tS (sleeping)\nVmRSS:\t17 kB\n"
        for error_number in (errno.EACCES, errno.EPERM, errno.EIO):
            with self.subTest(errno=error_number):
                def read(path, **_kwargs):
                    if str(path).endswith("/io"):
                        raise OSError(error_number, "sampling failure", str(path))
                    return live

                with patch.object(Path, "read_text", read), self.assertRaises(OSError) as failure:
                    process_snapshot(5697)
                self.assertEqual(failure.exception.errno, error_number)

    def test_live_rss_is_required_and_successful_snapshot_retains_values(self):
        def read(path, **_kwargs):
            return "State:\tR (running)\nVmRSS:\t17 kB\n" if str(path).endswith("/status") else "rchar: 33\nread_bytes: 44\n"

        with patch.object(Path, "read_text", read):
            snapshot = process_snapshot(5697)
        self.assertTrue(snapshot["valid"])
        self.assertEqual(snapshot["memory"]["VmRSS_bytes"], 17 * 1024)
        self.assertEqual(snapshot["io"], {"rchar": 33, "read_bytes": 44})
        with patch.object(Path, "read_text", return_value="State:\tR (running)\n"):
            unavailable = process_snapshot(5697)
        self.assertTrue(unavailable["rss_pending"])
        self.assertEqual(unavailable["memory_error"]["initial_status"], "State:\tR (running)\n")
        self.assertEqual(unavailable["memory_error"]["rechecked_status"], "State:\tR (running)\n")
        with patch.object(Path, "read_text", side_effect=PermissionError(errno.EACCES, "status denied")), self.assertRaises(PermissionError):
            process_snapshot(5697)

    def test_missing_rss_exit_race_requires_a_fresh_terminal_state(self):
        for state in ("Z", "X", "disappeared"):
            fresh = FileNotFoundError() if state == "disappeared" else f"State:\t{state} (exited)\n"
            with self.subTest(state=state), patch.object(Path, "read_text", side_effect=["State:\tR (running)\n", fresh]):
                snapshot = process_snapshot(3333)
            self.assertFalse(snapshot["valid"])
            self.assertEqual(snapshot["memory"], {})
            self.assertIsNone(snapshot["io"])
            self.assertEqual(snapshot["memory_error"]["state"], state)
            self.assertEqual(snapshot["memory_error"]["initial_state"], "R")
        with patch.object(Path, "read_text", side_effect=["State:\tR (running)\n", "State:\tS (sleeping)\n"]):
            unavailable = process_snapshot(3333)
        self.assertTrue(unavailable["rss_pending"])
        self.assertFalse(unavailable["valid"])

    def test_final_budget_counters_catch_exit_between_sampling_polls(self):
        mib = 1024 * 1024
        budget = SimpleNamespace(phase_seconds=600, rss_mib=8, disk_mib=12, cohort_disk_mib=24, minimum_free_mib=2)
        observed = {
            "finished_monotonic": 1600, "wall_seconds": 600,
            "time_max_process_rss_bytes": 8 * mib, "sample_allocated_bytes": 12 * mib,
            "cohort_allocated_bytes": 24 * mib, "filesystem_free_bytes": 2 * mib,
        }
        self.assertIsNone(final_budget_reason(None, observed, budget, 2000))
        violations = (
            ("wall_seconds", 600.001, "phase_wall_time_budget"),
            ("finished_monotonic", 2000.001, "cohort_wall_time_budget"),
            ("time_max_process_rss_bytes", 8 * mib + 1, "time_max_process_rss_budget"),
            ("time_max_process_rss_bytes", None, "final_resource_evidence_missing"),
            ("sample_allocated_bytes", 12 * mib + 1, "sample_allocated_disk_budget"),
            ("cohort_allocated_bytes", 24 * mib + 1, "cohort_allocated_disk_budget"),
            ("filesystem_free_bytes", 2 * mib - 1, "filesystem_free_space_guard"),
        )
        for field, value, reason in violations:
            with self.subTest(field=field, value=value):
                self.assertEqual(final_budget_reason(None, {**observed, field: value}, budget, 2000), reason)
        self.assertEqual(final_budget_reason("sampled_process_tree_rss_budget", {}, budget, 2000), "sampled_process_tree_rss_budget")

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

    def test_partial_stage_evidence_is_summarized_without_reading_skipped_driver(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            sample = root / "samples" / "partial"
            sample.mkdir(parents=True)
            manifest = {
                "label": "baseline", "operations": 256, "status": "incomplete", "mode": "timing",
                "stages": {
                    "build": {"status": "passed", "driver": {"latencies": {"append": {"median_ns": 7}}}},
                    "reopen": {"status": "incomplete", "reason": "phase_wall_time_budget"},
                    "recovery": {"status": "not_executed", "reason": "prerequisite_reopen_incomplete"},
                },
            }
            (sample / "manifest.json").write_text(json.dumps(manifest), encoding="utf-8")
            result = aggregate(root)
            self.assertFalse(result["all_requested_samples_passed"])
            self.assertEqual(len(result["timing_variation"]), 1)
            self.assertEqual(result["timing_variation"][0]["stage"], "build")

    def test_cancelled_passed_prefix_cannot_make_requested_matrix_green(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            requests = [{"sample": label, "label": label, "operations": 256, "repeat": 1, "mode": "timing"} for label in ("baseline", "candidate")]
            plan = {"cohort_complete": False, "samples": requests}
            (root / "requested.json").write_text(json.dumps(plan), encoding="utf-8")
            for index, request in enumerate(requests):
                sample = root / "samples" / request["sample"]
                sample.mkdir(parents=True)
                manifest = {key: request[key] for key in ("label", "operations", "repeat", "mode")}
                manifest.update(status="passed", stages={stage: {"status": "passed", "driver": {"latencies": {}}} for stage in ("build", "reopen", "recovery")})
                (sample / "manifest.json").write_text(json.dumps(manifest), encoding="utf-8")
                summary = aggregate(root)
                self.assertFalse(summary["all_requested_samples_passed"])
                if index == 0:
                    self.assertEqual(summary["missing_requested_samples"], ["candidate"])
            plan["cohort_complete"] = True
            (root / "requested.json").write_text(json.dumps(plan), encoding="utf-8")
            self.assertTrue(aggregate(root)["all_requested_samples_passed"])


if __name__ == "__main__":
    unittest.main()
