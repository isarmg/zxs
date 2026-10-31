"""Growth admission rejects incomplete evidence and unsafe resource forecasts."""

import copy
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools/history-scale-bench"))
from controller import BASELINE, PROFILES
import growth
import run


def write(root, name, value):
    path = root / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value), encoding="utf-8")


def policy():
    return {"schema_version": 1, "source_sha": "d" * 40, "source_tree": "e" * 40, "binary_sha256": "a" * 64,
            "build_run_id": "123", "build_carrier_sha": "b" * 40,
            "boundary_run_id": "124", "boundary_carrier_sha": "c" * 40}


def environment(profile="boundaries", run_id="124", carrier="c" * 40):
    p = policy()
    return {"validation_profile": profile, "source_matrix": ["candidate"],
            "sources": {"candidate": {"head": p["source_sha"], "precompiled_provenance": {"binary_sha256": p["binary_sha256"]}}},
            "expected_build_identity": {"GITHUB_RUN_ID": p["build_run_id"], "GITHUB_SHA": p["build_carrier_sha"]},
            "observer_identity": {"GITHUB_RUN_ID": run_id, "GITHUB_SHA": carrier}}


def metadata(run_id="124", carrier="c" * 40):
    return {"id": int(run_id), "head_sha": carrier, "path": ".github/workflows/history-scale.yml",
            "event": "workflow_dispatch", "status": "completed", "conclusion": "success"}


def sample(size, mode="timing", repeat=1):
    stages = {}
    for name, seconds in (("build", 4), ("reopen", 6), ("recovery", 2)):
        stages[name] = {"status": "passed", "supervisor": {
            "returncode": 0, "monitor_error": None, "finalization_errors": [], "budget_stop_reason": None,
            "wall_seconds": seconds * size / 1025, "time_max_process_rss_bytes": 4 * 1048576,
            "sampled_tree_peak_rss_bytes": 5 * 1048576, "disk_samples": [],
            "final_budget_observation": {"sample_allocated_bytes": size * 16384}},
            "driver": {"configuration": {"fixture_mode": "fixed-body", "query_protocol": "separate-outcome-health"},
                       "telemetry": [{"vmrss_kib": 4000}],
                       "completion": {"verified": True, "operations": size, "actual_total_receipts": size, "actual_total_events": size}}}
        if mode == "counts":
            stages[name]["trace"] = {"complete": True}
    return {"label": "candidate", "operations": size, "mode": mode, "repeat": repeat, "status": "passed", "stages": stages}


def summary(samples):
    return {"cohort_complete": True, "requested_matrix_matches": True, "all_requested_samples_passed": True, "samples": samples}


class GrowthTests(unittest.TestCase):
    def setUp(self):
        # Admission/resource unit tests isolate the existing strict protocol
        # reader; its full ordered protocol is exercised in test_tooling.py.
        self.reader = patch("growth.summarize_driver", side_effect=lambda path, *_args, **_kwargs: json.loads(path.read_text()))
        self.reader_mock = self.reader.start()
        self.addCleanup(self.reader.stop)

    def raw(self, root, profile, value):
        for row in value["samples"]:
            row.update(sha=policy()["source_sha"], binary_sha256=policy()["binary_sha256"],
                       profile=profile, query_protocol="separate-outcome-health",
                       total_sample_inventory={"allocated_bytes": row["operations"] * 17000})
            for name, stage in row["stages"].items():
                path = f"{profile}/samples/{row['operations']:06d}-{row['mode']}-{row['repeat']:02d}-candidate/{name}.driver.jsonl"
                write(root, path, stage["driver"])
        return value

    def fixture(self, root):
        p = policy()
        write(root, "policy.json", p)
        write(root, "basis-run.json", metadata())
        write(root, "prior-run.json", metadata())
        write(root, "basis/boundaries/environment.json", environment())
        basis = summary([sample(n) for n in (255, 256, 257, 1023, 1024, 1025)] + [sample(1025, "counts")])
        self.raw(root / "basis", "boundaries", basis)
        write(root, "basis/boundaries/summary.json", basis)
        plan = {"schema_version": 1, "baseline_sha": BASELINE, "profiles": list(PROFILES),
                "reviewed_candidate_sha": p["source_sha"], "frozen_candidate_sha": p["source_sha"], "ten_thousand_review": None}
        return plan, p, basis

    def test_admits_only_explicit_untraced_growth_after_complete_boundaries(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            plan, p, _ = self.fixture(root)
            settings, review = growth.admit(plan, "growth-2048", growth.load_policy(root / "policy.json"), root, "124", "c" * 40)
            self.assertTrue(review["admitted"])
            self.assertEqual(settings["sizes"], [2048])
            self.assertEqual(settings["counts_repeats"], 0)
            self.assertFalse(review["coverage"]["counts"])
            self.assertEqual(self.reader_mock.call_count, 21)
            self.assertTrue(review["reviewed_at_utc"].endswith("+00:00"))
            with self.assertRaisesRegex(ValueError, "frozen"):
                growth.admit({**plan, "frozen_candidate_sha": None}, "growth-2048", p, root, "124", "c" * 40)

    def test_rejects_missing_failed_duplicated_or_unobserved_predecessors(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            plan, p, basis = self.fixture(root)
            changes = []
            for field in ("cohort_complete", "requested_matrix_matches", "all_requested_samples_passed"):
                changes.append({**basis, field: False})
            changes.append({**basis, "samples": basis["samples"][:-1]})
            for field, value in (("returncode", 3), ("monitor_error", {"type": "PermissionError"}),
                                 ("budget_stop_reason", "phase_wall_time_budget"), ("time_max_process_rss_bytes", 0)):
                bad = copy.deepcopy(basis)
                bad["samples"][0]["stages"]["reopen"]["supervisor"][field] = value
                changes.append(bad)
            bad = copy.deepcopy(basis)
            bad["samples"][0]["stages"]["recovery"]["driver"]["completion"]["actual_total_receipts"] -= 1
            changes.append(bad)
            for changed in changes:
                write(root, "basis/boundaries/summary.json", changed)
                with self.assertRaises(ValueError):
                    growth.admit(plan, "growth-2048", p, root, "124", "c" * 40)

    def test_rejects_changed_sample_identity_and_complete_without_raw_protocol(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            plan, p, basis = self.fixture(root)
            for field, value in (("sha", "f" * 40), ("binary_sha256", "f" * 64), ("profile", "different")):
                changed = copy.deepcopy(basis)
                changed["samples"][0][field] = value
                write(root, "basis/boundaries/summary.json", changed)
                with self.assertRaisesRegex(ValueError, "sample source"):
                    growth.admit(plan, "growth-2048", p, root, "124", "c" * 40)
            write(root, "basis/boundaries/summary.json", basis)
            self.reader.stop()
            with self.assertRaises(ValueError):
                growth.admit(plan, "growth-2048", p, root, "124", "c" * 40)

    def test_rejects_wrong_source_binary_observer_and_unfinished_run(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            plan, p, _ = self.fixture(root)
            for field, value in (("source_sha", "f" * 40), ("binary_sha256", "f" * 64)):
                with self.subTest(field=field), self.assertRaises(ValueError):
                    growth.admit(plan, "growth-2048", {**p, field: value}, root, "124", "c" * 40)
            write(root, "prior-run.json", {**metadata(), "status": "in_progress"})
            with self.assertRaises(ValueError):
                growth.admit(plan, "growth-2048", p, root, "124", "c" * 40)
            write(root, "prior-run.json", metadata())
            env = environment(carrier="f" * 40)
            write(root, "basis/boundaries/environment.json", env)
            with self.assertRaisesRegex(ValueError, "observer"):
                growth.admit(plan, "growth-2048", p, root, "124", "c" * 40)

    def test_forecast_retains_both_rounds_and_rejects_unsafe_time_rss_and_disk(self):
        history = [{"size": 1025, "wall": {k: 100 for k in growth.STAGES}, "rss": 1048576, "disk": 1048576}]
        forecast = growth.project(history, 2048, 2)
        self.assertGreater(forecast["cohort_seconds"], 2 * sum(forecast["phase_seconds"].values()))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            plan, p, basis = self.fixture(root)
            for metric, value in (("wall_seconds", 600), ("time_max_process_rss_bytes", 2 * 1024**3)):
                bad = copy.deepcopy(basis)
                for row in bad["samples"]:
                    for stage in row["stages"].values():
                        stage["supervisor"][metric] = value
                write(root, "basis/boundaries/summary.json", bad)
                _, review = growth.admit(plan, "growth-2048", p, root, "124", "c" * 40)
                self.assertFalse(review["admitted"])
            bad = copy.deepcopy(basis)
            for row in bad["samples"]:
                for stage in row["stages"].values():
                    stage["supervisor"]["final_budget_observation"]["sample_allocated_bytes"] = 4 * 1024**3
            write(root, "basis/boundaries/summary.json", bad)
            self.assertFalse(growth.admit(plan, "growth-2048", p, root, "124", "c" * 40)[1]["admitted"])

    def test_optional_checkpoint_transition_uses_cold_admission_envelope(self):
        history = [{"size": 1024, "wall": {"build": 20, "reopen": 20, "recovery": 2}, "rss": 1024, "disk": 1024},
                   {"size": 2048, "wall": {"build": 67, "reopen": 73, "recovery": 72}, "rss": 2048, "disk": 2048}]
        forecast = growth.project(history, 4096, 1)
        self.assertEqual(forecast["phase_seconds"]["reopen"], forecast["phase_seconds"]["recovery"])
        self.assertGreater(forecast["phase_seconds"]["recovery"], 72 * 2 * growth.SAFETY)
        self.assertLess(forecast["phase_seconds"]["recovery"], 600)
        self.assertEqual(history[0]["wall"]["recovery"], 2)

    def test_measured_replay_growth_keeps_complete_cohort_inside_hosted_limit(self):
        history = [{"size": 2048, "wall": {"build": 66.862511334, "reopen": 73.073033687, "recovery": 71.920595386},
                    "rss": 37126144, "disk": 242044928},
                   {"size": 4096, "wall": {"build": 355.875552146, "reopen": 590.111177936, "recovery": 577.497994163},
                    "rss": 57937920, "disk": 444223488}]
        forecast = growth.project(history, 5000, 2)
        self.assertLess(forecast["cohort_seconds"], growth.TIERS["growth-5000"][4])
        self.assertGreater(growth.project(history, 10000, 1)["cohort_seconds"], 6 * 3600)
        self.assertNotIn("growth-10000", growth.TIERS)
        self.assertNotIn("growth-100000", growth.TIERS)
        # A small next step uses both rounds' worst actual cost, but does not
        # fit a noisy new exponent from the narrow 4096 -> 5000 ratio.
        later = {"size": 5000, "wall": {"build": 500, "reopen": 900, "recovery": 880},
                 "rss": 65000000, "disk": 550000000}
        extended = history + [later]
        self.assertEqual(growth.exponent(history, "wall", "reopen"), growth.exponent(extended, "wall", "reopen"))
        self.assertGreater(growth.project(extended, 8192, 1)["phase_seconds"]["recovery"], 900 * growth.SAFETY)

    def test_repeated_tier_uses_worst_actual_stage_and_full_sample(self):
        first, second = sample(5000, repeat=1), sample(5000, repeat=2)
        first["total_sample_inventory"] = {"allocated_bytes": 90000000}
        second["total_sample_inventory"] = {"allocated_bytes": 80000000}
        first["stages"]["build"]["supervisor"]["wall_seconds"] = 100
        second["stages"]["build"]["supervisor"]["wall_seconds"] = 125
        actual = growth.observed([first, second], 5000)
        self.assertEqual(actual["wall"]["build"], 125)
        self.assertEqual(actual["disk"], 90000000)

    def test_counts_projection_binds_native_predecessor_and_uses_cold_open(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            plan, p, basis = self.fixture(root)
            _, first = growth.admit(plan, "growth-2048", p, root, "124", "c" * 40)
            write(root, "prior-run.json", metadata("125", "f" * 40))
            env = environment("growth-2048", "125", "f" * 40)
            env["growth_review"] = first
            write(root, "prior/growth-2048/environment.json", env)
            native = self.raw(root / "prior", "growth-2048", summary([sample(2048)]))
            write(root, "prior/growth-2048/summary.json", native)
            # Budget admission uses the same strict raw reader as timing;
            # isolate only syscall sampling/trace-length observations here.
            for stage in growth.STAGES:
                path = root / f"basis/boundaries/samples/001025-counts-01-candidate/{stage}.strace.txt"
                path.write_text("fixture syscall\n")
            with patch("growth.io_observation", return_value={"sampled_syscr_peak": 100, "rchar": 1000}):
                settings, review = growth.admit(plan, "growth-counts-2048", p, root, "125", "f" * 40)
            self.assertTrue(review["admitted"])
            self.assertEqual(settings["repeats"], 0)
            self.assertEqual(settings["counts_repeats"], 1)
            self.assertEqual(review["coverage"]["timing_counterpart_run_id"], "125")
            self.assertFalse(review["coverage"]["timing"])
            self.assertEqual(review["forecast"]["io_projection"]["recovery"]["cold_reference_stage"], "reopen")
            self.assertEqual(review["budgets"]["disk_mib"], 12288)

    def test_complete_stages_do_not_hide_late_trace_parse_or_copy(self):
        args = SimpleNamespace(output_root=Path("/unused"), cohort_started=0, cohort_deadline=10,
                               disk_mib=64, cohort_disk_mib=128, minimum_free_mib=1,
                               growth_review={"coverage": {"counts": True}, "admitted": True})
        with patch("run.aggregate", return_value={"all_requested_samples_passed": True, "samples": []}), \
             patch("run.inventory", return_value={"allocated_bytes": 0}), \
             patch("run.shutil.disk_usage", return_value=SimpleNamespace(free=1024**3)), \
             patch("run.time.monotonic", return_value=11):
            result = run.growth_summary(args)
        self.assertFalse(result["all_requested_samples_passed"])
        self.assertTrue(result["growth_cohort_budget_exceeded"])

    def test_complete_stages_do_not_hide_disk_or_free_after_oracle_copy(self):
        args = SimpleNamespace(output_root=Path("/unused"), cohort_started=0, cohort_deadline=10,
                               disk_mib=64, cohort_disk_mib=128, minimum_free_mib=1,
                               growth_review={"coverage": {"counts": True}, "admitted": True})
        for sample_bytes, cohort_bytes, free_bytes, reason in (
            (65 * 1048576, 1048576, 1024**3, "final_complete_sample_disk_budget"),
            (1048576, 129 * 1048576, 1024**3, "final_complete_cohort_disk_budget"),
            (1048576, 1048576, 0, "final_free_space_guard"),
        ):
            with self.subTest(reason=reason), patch("run.aggregate", return_value={"all_requested_samples_passed": True,
                 "samples": [{"total_sample_inventory": {"allocated_bytes": sample_bytes}}]}), \
                 patch("run.inventory", return_value={"allocated_bytes": cohort_bytes}), \
                 patch("run.shutil.disk_usage", return_value=SimpleNamespace(free=free_bytes)), \
                 patch("run.time.monotonic", return_value=5):
                result = run.growth_summary(args)
                self.assertFalse(result["all_requested_samples_passed"])
                self.assertIn(reason, result["growth_final_budget_reasons"])

    def test_requires_exact_immediate_predecessor_and_binds_resource_history(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            plan, p, _ = self.fixture(root)
            _, first = growth.admit(plan, "growth-2048", p, root, "124", "c" * 40)
            write(root, "prior-run.json", metadata("125", "f" * 40))
            env = environment("growth-2048", "125", "f" * 40)
            env["growth_review"] = first
            write(root, "prior/growth-2048/environment.json", env)
            prior = self.raw(root / "prior", "growth-2048", summary([sample(2048)]))
            write(root, "prior/growth-2048/summary.json", prior)
            self.assertTrue(growth.admit(plan, "growth-4096", p, root, "125", "f" * 40)[1]["admitted"])
            env["growth_review"]["resource_history"][0]["disk"] += 1
            write(root, "prior/growth-2048/environment.json", env)
            with self.assertRaisesRegex(ValueError, "resource history"):
                growth.admit(plan, "growth-4096", p, root, "125", "f" * 40)

    def test_8192_requires_completed_5000_both_rounds_and_entire_history(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            plan, p, _ = self.fixture(root)
            prior_run, prior_sha = "124", "c" * 40
            for profile, run_id in (("growth-2048", "125"), ("growth-4096", "126"), ("growth-5000", "127")):
                _, review = growth.admit(plan, profile, p, root, prior_run, prior_sha)
                self.assertTrue(review["admitted"])
                size, _, repeats, _, _ = growth.TIERS[profile]
                env = environment(profile, run_id, "f" * 40)
                env["growth_review"] = review
                write(root, f"prior/{profile}/environment.json", env)
                value = self.raw(root / "prior", profile, summary([sample(size, repeat=r) for r in range(1, repeats + 1)]))
                write(root, f"prior/{profile}/summary.json", value)
                write(root, "prior-run.json", metadata(run_id, "f" * 40))
                prior_run, prior_sha = run_id, "f" * 40
            settings, review = growth.admit(plan, "growth-8192", p, root, prior_run, prior_sha)
            self.assertTrue(review["admitted"])
            self.assertEqual(settings["sizes"], [8192])
            self.assertEqual([row["size"] for row in review["resource_history"]], [257, 1025, 2048, 4096, 5000])
            value["samples"].pop()
            write(root, "prior/growth-5000/summary.json", value)
            with self.assertRaisesRegex(ValueError, "matrix"):
                growth.admit(plan, "growth-8192", p, root, prior_run, prior_sha)

    def test_insufficient_initial_space_keeps_denial_and_never_creates_a_pool(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "output"
            args = SimpleNamespace(output_root=root, revisions={"candidate": "d" * 40}, sizes=[2048], repeats=1,
                                   counts_repeats=0, counts_sizes=[], minimum_free_mib=8192,
                                   growth_review={"admitted": True, "forecast": {"cohort_bytes": 1024**3}, "denial_reasons": [],
                                                  "coverage": {"counts": False}})
            with patch("run.arguments", return_value=args), patch.dict("run.os.environ", {"GITHUB_ACTIONS": "true"}), \
                 patch("run.environment", return_value={"growth_review": args.growth_review}), \
                 patch("run.shutil.which", return_value="/fixture/strace"), \
                 patch("run.shutil.disk_usage", return_value=SimpleNamespace(free=1024**3)), \
                 patch("run.run_sample") as execute:
                self.assertEqual(run.main(), 2)
            execute.assert_not_called()
            self.assertFalse((root / "samples").exists())
            result = json.loads((root / "summary.json").read_text())
            self.assertFalse(result["all_requested_samples_passed"])
            self.assertFalse(result["all_growth_capacity_metrics_passed"])
            self.assertFalse(json.loads((root / "growth-review.json").read_text())["admitted"])
