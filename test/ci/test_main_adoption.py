"""Exact-SHA adoption, using a captured real full CI attempt as evidence.

The fixture is a field projection of public API run 41031345216, attempt 1,
including genuinely skipped conditional setup steps and original artifact times.
No test makes network requests.
"""
import copy
from datetime import datetime, timedelta, timezone
import importlib.util
import json
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("main_adoption", ROOT / ".github/scripts/check-main-adoption.py")
adoption = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(adoption)
FIXTURE = json.loads((ROOT / "test/ci/fixtures/main-adoption-run.json").read_text())
NOW = datetime(2026, 10, 30, 10, tzinfo=timezone.utc)


class Evidence(unittest.TestCase):
    def setUp(self):
        self.run = copy.deepcopy(FIXTURE["run"])
        self.listed = copy.deepcopy(self.run)
        self.jobs = copy.deepcopy(FIXTURE["jobs"])
        self.artifacts = copy.deepcopy(FIXTURE["artifacts"])
        sha = self.run["head_sha"]
        self.environment = {
            "GITHUB_EVENT_NAME": "push", "GITHUB_REF": "refs/heads/main",
            "GITHUB_SHA": sha, "GITHUB_WORKFLOW_SHA": sha,
            "GITHUB_WORKFLOW_REF": "isarmg/zxs/.github/workflows/ci.yml@refs/heads/main",
            "GITHUB_SERVER_URL": "https://github.com", "GITHUB_REPOSITORY": "isarmg/zxs",
            "GITHUB_REPOSITORY_ID": "1424437655", "GITHUB_RUN_ID": "41039999999",
            "GITHUB_RUN_ATTEMPT": "1",
        }
        self.event = {
            "ref": "refs/heads/main", "before": "f" * 40, "after": sha,
            "head_commit": {"id": sha}, "repository": copy.deepcopy(self.run["repository"]),
            "forced": False, "created": False, "deleted": False,
        }
        self.current = dict(self.run, id=41039999999, head_branch="main", status="in_progress", conclusion=None)
        self.candidates = [self.listed]
        self.calls = []
        self.fail_call = None
        self.malformed_call = None
        self.latest = None

    def get(self, path):
        self.calls.append(path)
        if len(self.calls) == self.fail_call:
            raise OSError("injected public API failure")
        if len(self.calls) == self.malformed_call:
            return None
        if path == f"/actions/runs/{self.current['id']}":
            return self.current
        if path.startswith("/actions/workflows/389550847/runs?"):
            self.assertIn("head_sha=" + self.environment["GITHUB_SHA"], path)
            self.assertIn("event=push", path)
            self.assertIn("status=success", path)
            self.assertIn("per_page=3", path)
            return {"workflow_runs": self.candidates}
        if path == f"/actions/runs/{self.run['id']}":
            return self.latest if self.latest is not None and len(self.calls) == 6 else self.run
        if path == f"/actions/runs/{self.run['id']}/attempts/{self.run['run_attempt']}/jobs?per_page=100":
            return self.jobs
        if path == f"/actions/runs/{self.run['id']}/artifacts?per_page=100":
            return self.artifacts
        self.fail(f"unexpected public API read: {path}")

    def decide(self):
        return adoption.find_adoption(self.environment, self.event, self.get, NOW)

    def assert_full(self):
        run, reason = self.decide()
        self.assertIsNone(run, reason)
        self.assertTrue(reason)
        self.assertLessEqual(len(self.calls), 2 + 4 * adoption.MAX_CANDIDATES)

    def test_real_complete_exact_match_adopts_original_attempt(self):
        run, reason = self.decide()
        self.assertEqual(run["id"], 41031345216, reason)
        self.assertEqual(run["run_attempt"], 1)
        self.assertEqual(run["head_sha"], self.environment["GITHUB_SHA"])
        self.assertEqual(len(self.calls), 6)
        self.assertIn("/attempts/1/jobs?per_page=100", self.calls[3])
        self.assertEqual(self.calls[2], self.calls[5])

    def test_full_candidate_with_new_reporting_steps_is_still_full_evidence(self):
        for job in self.jobs["jobs"]:
            if job["name"] == "Static workflow and tooling regressions (no Rust)":
                job["steps"].append({"name": "Check exact-SHA main adoption", "status": "completed", "conclusion": "success"})
            else:
                job["steps"].append({"name": adoption.REPORT_STEP, "status": "completed", "conclusion": "skipped"})
        run, reason = self.decide()
        self.assertEqual(run["id"], self.run["id"], reason)

    def test_candidate_branches_pr_and_manual_runs_always_validate_without_api(self):
        for event, ref in (("push", "refs/heads/work/candidate"), ("push", "refs/tags/v1"),
                           ("pull_request", "refs/pull/1/merge"), ("workflow_dispatch", "refs/heads/main")):
            with self.subTest(event=event, ref=ref):
                self.environment.update(GITHUB_EVENT_NAME=event, GITHUB_REF=ref)
                self.assert_full()
                self.assertEqual(self.calls, [])

    def test_direct_untested_main_push_selects_full(self):
        self.candidates = []
        self.assert_full()
        self.assertEqual(len(self.calls), 2)

    def test_uncertain_environment_and_changed_push_sha_select_full(self):
        for key, value in (
            ("GITHUB_SHA", "7ed4e00"), ("GITHUB_WORKFLOW_SHA", "a" * 40),
            ("GITHUB_REPOSITORY", "someone/zxs"), ("GITHUB_REPOSITORY_ID", "1"),
            ("GITHUB_SERVER_URL", "https://elsewhere.example"),
            ("GITHUB_WORKFLOW_REF", "isarmg/zxs/.github/workflows/other.yml@refs/heads/main"),
        ):
            with self.subTest(key=key):
                self.setUp()
                self.environment[key] = value
                self.assert_full()
                self.assertEqual(self.calls, [])
        for key, value in (("after", "b" * 40), ("forced", True), ("deleted", True),
                           ("created", True), ("before", "0" * 40), ("repository", None)):
            with self.subTest(key=key):
                self.setUp()
                self.event[key] = value
                self.assert_full()
                self.assertEqual(self.calls, [])

    def test_foreign_sha_workflow_pr_merge_and_main_run_are_rejected(self):
        for key, value in (
            ("head_sha", "a" * 40), ("head_sha", FIXTURE["run"]["head_sha"][:8]),
            ("head_commit", {"id": "a" * 40}), ("event", "pull_request"),
            ("head_branch", "main"), ("head_branch", "refs/pull/1/merge"),
            ("workflow_id", 123), ("path", ".github/workflows/release.yml"),
            ("head_repository", {"id": 1, "full_name": "someone/zxs"}),
            ("repository", {"id": 1, "full_name": "someone/zxs"}),
            ("status", "in_progress"), ("conclusion", "cancelled"),
        ):
            with self.subTest(key=key, value=value):
                self.setUp()
                self.listed[key] = self.run[key] = value
                self.assert_full()

    def test_current_and_candidate_changes_during_lookup_select_full(self):
        for target, key, value in (("current", "head_sha", "a" * 40),
                                    ("current", "run_attempt", 2),
                                    ("run", "run_attempt", 2),
                                    ("run", "head_repository", None)):
            with self.subTest(target=target, key=key):
                self.setUp()
                getattr(self, target)[key] = value
                self.assert_full()
        for key, value in (("run_attempt", 2), ("status", "queued"), ("head_sha", "a" * 40)):
            with self.subTest(recheck=key):
                self.setUp()
                self.latest = dict(self.run, **{key: value})
                self.assert_full()

    def test_every_required_job_and_execution_step_must_really_succeed(self):
        for index, original in enumerate(FIXTURE["jobs"]["jobs"]):
            for conclusion in ("skipped", "cancelled", "failure"):
                with self.subTest(job=original["name"], conclusion=conclusion):
                    self.setUp()
                    self.jobs["jobs"][index]["conclusion"] = conclusion
                    self.assert_full()
            for name in adoption.JOB_STEPS[original["name"]]:
                with self.subTest(job=original["name"], step=name):
                    self.setUp()
                    step = next(s for s in self.jobs["jobs"][index]["steps"] if s["name"] == name)
                    step["conclusion"] = "skipped"
                    self.assert_full()

    def test_incomplete_duplicate_or_wrong_attempt_jobs_select_full(self):
        for change in ("missing", "duplicate", "foreign_sha", "wrong_attempt", "invalid_attempt", "missing_step", "unknown_skipped"):
            with self.subTest(change=change):
                self.setUp()
                job = self.jobs["jobs"][0]
                if change == "missing":
                    self.jobs["jobs"].pop()
                elif change == "duplicate":
                    self.jobs["jobs"][-1] = copy.deepcopy(job)
                elif change == "foreign_sha":
                    job["head_sha"] = "a" * 40
                elif change == "wrong_attempt":
                    job["run_attempt"] = 2
                elif change == "invalid_attempt":
                    job["run_attempt"] = True
                elif change == "missing_step":
                    job["steps"] = []
                else:
                    job["steps"].append({"name": "New source check", "status": "completed", "conclusion": "skipped"})
                self.assert_full()

    def test_previous_reuse_only_success_cannot_become_source_evidence(self):
        # Even if all lightweight reporting jobs succeeded, skipped source steps
        # must not authorize chaining adoption through that run.
        for job in self.jobs["jobs"]:
            if job["name"] not in {"Static workflow and tooling regressions (no Rust)", "Tests (security)"}:
                for step in job["steps"]:
                    step["conclusion"] = "skipped"
        self.assert_full()

    def test_missing_expired_or_foreign_artifacts_select_full(self):
        for index, item in enumerate(FIXTURE["artifacts"]["artifacts"]):
            with self.subTest(missing=item["name"]):
                self.setUp()
                self.artifacts["artifacts"].pop(index)
                self.assert_full()
        for key, value in (("expired", True), ("expires_at", "2026-10-29T00:00:00Z"),
                           ("digest", None), ("size_in_bytes", 0), ("created_at", "2026-10-29T00:00:00Z"),
                           ("workflow_run", {"id": 1})):
            with self.subTest(key=key):
                self.setUp()
                self.artifacts["artifacts"][0][key] = value
                self.assert_full()

    def test_full_rerun_requires_jobs_and_artifacts_from_that_attempt(self):
        self.run["run_attempt"] = self.listed["run_attempt"] = 2
        for job in self.jobs["jobs"]:
            job["run_attempt"] = 2
            for step in job["steps"]:
                for key in ("started_at", "completed_at"):
                    step[key] = (adoption.timestamp(step[key]) + timedelta(minutes=10)).isoformat()
        self.assert_full()  # Original attempt's artifacts cannot fill the gap.
        self.calls = []
        for item in self.artifacts["artifacts"]:
            for key in ("created_at", "updated_at"):
                item[key] = (adoption.timestamp(item[key]) + timedelta(minutes=10)).isoformat()
        run, reason = self.decide()
        self.assertEqual(run["run_attempt"], 2, reason)

    def test_api_failures_or_malformed_responses_at_each_read_select_full(self):
        for call in range(1, 7):
            for failure in ("fail_call", "malformed_call"):
                with self.subTest(call=call, failure=failure):
                    self.setUp()
                    setattr(self, failure, call)
                    self.assert_full()
                    self.assertEqual(len(self.calls), call)  # No retry.

    def test_lookup_does_not_page_or_accept_unbounded_evidence(self):
        self.candidates = [self.listed] * 4
        self.assert_full()
        self.assertEqual(len(self.calls), 2)


class WorkflowIntegration(unittest.TestCase):
    def jobs(self, name):
        source = (ROOT / ".github/workflows" / name).read_text()
        parts = re.split(r"^  ([A-Za-z0-9_-]+):\s*$", source.split("\njobs:\n", 1)[1], flags=re.MULTILINE)
        return dict(zip(parts[1::2], parts[2::2]))

    def test_existing_job_names_matrix_and_execution_contract_stay_complete(self):
        jobs = self.jobs("ci.yml")
        self.assertEqual(set(jobs), {"static", "tests", "web-production", "browser-smoke", "smb-sdk"})
        suites = re.search(r"suite: \[([^\]]+)\]", jobs["tests"])[1].split(", ")
        self.assertEqual(tuple(suites), adoption.SUITES)
        names = {
            "static": "Static workflow and tooling regressions (no Rust)",
            "web-production": "Production WASM build",
            "browser-smoke": "Extracted release browser smoke (Chrome)",
        }
        for key, name in names.items():
            self.assertIn(f"name: {name}\n", jobs[key])
            declared = set(re.findall(r"^      - name: (.+)$", jobs[key], re.MULTILINE))
            declared.discard(adoption.REPORT_STEP)
            declared.discard("Check exact-SHA main adoption")
            self.assertEqual(declared | {adoption.CHECKOUT}, adoption.JOB_STEPS[name])
        declared = set(re.findall(r"^      - name: (.+)$", jobs["tests"], re.MULTILINE)) - {adoption.REPORT_STEP}
        required = set().union(*(adoption.JOB_STEPS[f"Tests ({suite})"] for suite in suites))
        self.assertEqual(declared | {adoption.CHECKOUT}, required)
        sdk = self.jobs("smb-sdk.yml")["smb-sdk"]
        declared = set(re.findall(r"^      - name: (.+)$", sdk, re.MULTILINE)) - {adoption.REPORT_STEP}
        self.assertEqual(declared | {adoption.CHECKOUT}, adoption.JOB_STEPS["smb-sdk / Samba 4.23.0 real SDK ABI and FFI tests"])

    def test_failures_run_full_static_security_and_keep_original_artifacts(self):
        jobs = self.jobs("ci.yml")
        self.assertIn("steps.adoption.outcome == 'success' && steps.adoption.outputs.reuse-source || 'false'", jobs["static"])
        self.assertLess(jobs["static"].index("check.sh static"), jobs["static"].index("check-main-adoption.py"))
        self.assertIn("continue-on-error: true", jobs["static"])
        self.assertIn("needs.static.outputs.reuse-source != 'true' || matrix.suite == 'security'", jobs["tests"])
        for key in ("tests", "web-production", "browser-smoke"):
            self.assertNotRegex(jobs[key], r"(?m)^    if:")  # Keep every existing check/job.
            for step in re.split(r"^      - ", jobs[key], flags=re.MULTILINE)[1:]:
                if step.startswith("name: Report adopted source validation"):
                    self.assertIn("if: env.ZXS_RUN_SOURCE_VALIDATION != 'true'", step)
                elif step.startswith("name: Cache verified security scanner"):
                    self.assertIn("if: matrix.suite == 'security'", step)
                else:
                    self.assertIn("env.ZXS_RUN_SOURCE_VALIDATION == 'true'", step)
                self.assertNotIn("run-id:", step)  # No cross-run downloads or artifact relabelling.
        self.assertIn("needs: [static, web-production]", jobs["browser-smoke"])
        self.assertIn("needs.static.outputs.reuse-source == 'true' && needs.static.outputs.source-run-url || ''", jobs["smb-sdk"])
        release = (ROOT / ".github/workflows/release.yml").read_text()
        self.assertNotIn("source-validation-run-url", release)
        sdk = (ROOT / ".github/workflows/smb-sdk.yml").read_text()
        self.assertIn("default: ''", sdk)
        for step in re.split(r"^      - ", self.jobs("smb-sdk.yml")["smb-sdk"], flags=re.MULTILINE)[1:]:
            if not step.startswith("name: Report adopted source validation"):
                self.assertIn("if: inputs.source-validation-run-url == ''", step)


if __name__ == "__main__":
    unittest.main()
