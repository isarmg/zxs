"""The preparation workflow cannot accidentally start a scale acceptance run."""

from __future__ import annotations

import json
from pathlib import Path
import sys
import tempfile
import unittest

TOOLS = Path(__file__).resolve().parents[2] / "tools/history-scale-bench"
sys.path.insert(0, str(TOOLS))
from controller import BASELINE, PROFILES, build_source, load_plan, profile_settings, select, validate_build_jobs, validate_reuse_run, validate_reused_selftest


class ControllerTests(unittest.TestCase):
    def plan(self, **changes):
        plan = {
            "schema_version": 1, "baseline_sha": BASELINE,
            "reviewed_candidate_sha": "209f31a4122174129bcf40b56c0598ac61e66352",
            "frozen_candidate_sha": None, "ten_thousand_review": None, "profiles": list(PROFILES),
        }
        plan.update(changes)
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "plan.json"
            path.write_text(json.dumps(plan), encoding="utf-8")
            return load_plan(path)

    def test_preparation_only_selects_micro_fixture(self):
        plan = self.plan()
        selection = select(plan, "selftest")
        self.assertEqual(selection["matrix"], {"include": [{"profile": "small-selftest"}]})
        self.assertEqual(selection["settings"]["small-selftest"]["sizes"], [8])
        self.assertFalse(selection["candidate_frozen"])

    def test_scale_stays_locked_until_freeze(self):
        plan = self.plan()
        for name in PROFILES[1:]:
            with self.subTest(profile=name), self.assertRaisesRegex(ValueError, "final frozen SHA"):
                profile_settings(plan, name)
        with self.assertRaises(ValueError):
            select(plan, "acceptance")

    def test_freeze_has_to_match_source_review(self):
        with self.assertRaisesRegex(ValueError, "source-reviewed"):
            self.plan(frozen_candidate_sha="f" * 40)
        plan = self.plan(frozen_candidate_sha="209f31a4122174129bcf40b56c0598ac61e66352")
        self.assertEqual(len(select(plan, "acceptance")["matrix"]["include"]), 2)

    def test_10k_waits_for_initial_results_and_resource_review(self):
        frozen = "209f31a4122174129bcf40b56c0598ac61e66352"
        with self.assertRaisesRegex(ValueError, "result/resource review"):
            profile_settings(self.plan(frozen_candidate_sha=frozen), "10000")
        review = {"candidate_sha": frozen, "validation_run_id": "123", "reviewed_profiles": ["boundaries", "1000"],
                  "rationale": "Unit fixture: completed initial results and resource trends reviewed."}
        plan = self.plan(frozen_candidate_sha=frozen, ten_thousand_review=review)
        self.assertEqual(select(plan, "10000")["matrix"], {"include": [{"profile": "10000"}]})

    def test_scale_reuses_an_explicit_binary_build(self):
        sha = "a" * 40
        self.assertTrue(build_source("selftest", "", "", "123", sha)["build_required"])
        self.assertFalse(build_source("acceptance", "123", sha, "124", "b" * 40)["build_required"])
        for mode, run, carrier in (("acceptance", "", ""), ("10000", "", ""), ("selftest", "123", ""), ("acceptance", "0", sha)):
            with self.subTest(mode=mode, run=run), self.assertRaises(ValueError):
                build_source(mode, run, carrier, "124", "b" * 40)

    def test_failed_or_unfinished_selftest_cannot_release_scale(self):
        run = {"id": 123, "head_sha": "a" * 40, "path": ".github/workflows/history-scale.yml",
               "event": "workflow_dispatch", "status": "completed", "conclusion": "success"}
        validate_reuse_run(run, "123", "a" * 40)
        for change in ({"conclusion": "failure"}, {"status": "in_progress"}, {"head_sha": "b" * 40}, {"path": ".github/workflows/ci.yml"}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                validate_reuse_run({**run, **change}, "123", "a" * 40)

    def test_observer_repair_can_reuse_two_successful_builds(self):
        run = {"id": 123, "head_sha": "a" * 40, "path": ".github/workflows/history-scale.yml",
               "event": "workflow_dispatch", "status": "completed", "conclusion": "failure"}
        jobs = {"jobs": [{"name": name, "status": "completed", "conclusion": "success"}
                         for name in ("build (baseline)", "build (candidate)")]}
        validate_build_jobs(run, jobs, "123", "a" * 40)
        with self.assertRaises(ValueError):
            validate_reuse_run(run, "123", "a" * 40)
        jobs["jobs"][0]["conclusion"] = "failure"
        with self.assertRaisesRegex(ValueError, "both original"):
            validate_build_jobs(run, jobs, "123", "a" * 40)

    def test_original_selftest_needs_all_four_samples_and_twelve_stages(self):
        selection = {"mode": "selftest", "build_required": True, "build_run_id": "123", "build_carrier_sha": "a" * 40,
                     "baseline_sha": BASELINE, "candidate_sha": "b" * 40,
                     "matrix": {"include": [{"profile": "small-selftest"}]}}
        samples = [{"label": label, "mode": mode, "operations": 8, "status": "passed",
                    "stages": {stage: {"status": "passed"} for stage in ("build", "reopen", "recovery")}}
                   for label in ("baseline", "candidate") for mode in ("timing", "counts")]
        summary = {"all_requested_samples_passed": True, "cohort_complete": True, "requested_matrix_matches": True, "samples": samples}
        validate_reused_selftest(selection, summary, "123", "a" * 40, "b" * 40)
        validate_reused_selftest({**selection, "build_required": False}, summary, "123", "a" * 40, "b" * 40)
        for change in ({"samples": samples[:-1]}, {"cohort_complete": False}, {"all_requested_samples_passed": False}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                validate_reused_selftest(selection, {**summary, **change}, "123", "a" * 40, "b" * 40)
        with self.assertRaisesRegex(ValueError, "source revisions"):
            validate_reused_selftest(selection, summary, "123", "a" * 40, "c" * 40)

    def test_profiles_cannot_expand_to_100k(self):
        with self.assertRaisesRegex(ValueError, "100k"):
            self.plan(profiles=list(PROFILES) + ["100000"])
        with self.assertRaisesRegex(ValueError, "unsupported"):
            profile_settings(self.plan(), "100000")

    def test_refs_and_changed_baselines_are_rejected(self):
        for change in ({"reviewed_candidate_sha": "main"}, {"baseline_sha": "f" * 40}, {"schema_version": True}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.plan(**change)

    def test_duplicate_freeze_and_unknown_modes_are_rejected(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "plan.json"
            path.write_text('{"frozen_candidate_sha":null,"frozen_candidate_sha":"' + "f" * 40 + '"}')
            with self.assertRaisesRegex(ValueError, "duplicate"):
                load_plan(path)
        with self.assertRaisesRegex(ValueError, "mode"):
            select(self.plan(), "benchmark")


if __name__ == "__main__":
    unittest.main()
