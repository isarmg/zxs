"""Skipping is permitted only by exact success evidence and reviewed inputs."""
import copy
import http.client
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("selection", ROOT / ".github/scripts/select-ci-suites.py")
selection = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(selection)
BASE = "a" * 40
HEAD = "b" * 40


def evidence(base=BASE):
    run = {"id": 12, "run_attempt": 1, "head_sha": base, "head_branch": "main",
           "event": "push", "path": ".github/workflows/ci.yml", "status": "completed",
           "conclusion": "failure"}  # An unrelated failure is not a foundation failure.
    jobs = [{"id": 100 + i, "run_id": 12, "run_attempt": 1, "head_sha": base,
             "name": f"Tests ({suite})", "status": "completed", "conclusion": "success"}
            for i, suite in enumerate(selection.FOUNDATION)]
    return {"total_count": 1, "workflow_runs": [run]}, {"total_count": len(jobs), "jobs": jobs}


def fetcher(documents):
    def fetch(url):
        if "/jobs?" in url:
            return documents[1]
        return documents[0]
    return fetch


class Evidence(unittest.TestCase):
    def test_exact_five_jobs_allow_reuse_even_if_other_job_failed(self):
        result = selection.foundation_evidence(BASE, fetcher(evidence()))
        self.assertEqual(set(result["jobs"]), set(selection.FOUNDATION))
        self.assertEqual(result["run_id"], 12)

    def test_every_non_success_status_fails_closed(self):
        for conclusion in (None, "failure", "cancelled", "skipped", "neutral", "timed_out"):
            with self.subTest(conclusion=conclusion):
                docs = evidence()
                docs[1]["jobs"][1]["conclusion"] = conclusion
                with self.assertRaises(selection.FullRun):
                    selection.foundation_evidence(BASE, fetcher(docs))

    def test_missing_duplicate_similar_name_wrong_identity_or_attempt_rejected(self):
        for mutation in ("missing", "duplicate", "similar", "head", "run", "attempt", "running", "id", "bool"):
            with self.subTest(mutation=mutation):
                docs = evidence(); jobs = docs[1]["jobs"]
                if mutation == "missing": jobs.pop()
                elif mutation == "duplicate": jobs.append(copy.deepcopy(jobs[0]))
                elif mutation == "similar": jobs[0]["name"] += " reused"
                elif mutation == "head": jobs[0]["head_sha"] = HEAD
                elif mutation == "run": jobs[0]["run_id"] = 13
                elif mutation == "attempt": jobs[0]["run_attempt"] = 2
                elif mutation == "running": jobs[0]["status"] = "in_progress"
                elif mutation == "id": jobs[0]["id"] = 0
                else: jobs[0]["run_attempt"] = True
                docs[1]["total_count"] = len(jobs)
                with self.assertRaises(selection.FullRun):
                    selection.foundation_evidence(BASE, fetcher(docs))

    def test_wrong_run_identity_or_missing_base_run_rejected(self):
        for key, value in (("head_sha", HEAD), ("head_branch", "other"), ("event", "pull_request"),
                           ("path", ".github/workflows/other.yml"), ("status", "in_progress"), ("id", True)):
            docs = evidence(); docs[0]["workflow_runs"][0][key] = value
            with self.subTest(key=key), self.assertRaises(selection.FullRun):
                selection.foundation_evidence(BASE, fetcher(docs))
        with self.assertRaises(selection.FullRun):
            selection.foundation_evidence(BASE, fetcher(({"total_count": 0, "workflow_runs": []}, {})))

    def test_newer_pending_run_does_not_fall_back_to_old_success(self):
        for status in ("queued", "in_progress", "unknown"):
            docs = evidence()
            newer = dict(docs[0]["workflow_runs"][0], id=13, status=status)
            docs[0]["workflow_runs"].append(newer); docs[0]["total_count"] = 2
            with self.subTest(status=status), self.assertRaises(selection.FullRun):
                selection.foundation_evidence(BASE, fetcher(docs))
        docs = evidence(); docs[0]["workflow_runs"].append({}); docs[0]["total_count"] = 2
        with self.assertRaises(selection.FullRun):
            selection.foundation_evidence(BASE, fetcher(docs))

    def test_incomplete_pages_and_unknown_response_shapes_rejected(self):
        for index in (0, 1):
            for replacement in (None, [], {}, {"total_count": "5"}, {"total_count": True}):
                docs = list(evidence()); docs[index] = replacement
                with self.subTest(index=index, replacement=replacement), self.assertRaises(selection.FullRun):
                    selection.foundation_evidence(BASE, fetcher(docs))
            docs = evidence(); docs[index]["total_count"] = 101
            with self.assertRaises(selection.FullRun):
                selection.foundation_evidence(BASE, fetcher(docs))


class Selector(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(); self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.git("init", "-q"); self.git("config", "user.name", "Test")
        self.git("config", "user.email", "test@example.invalid")
        self.write("Cargo.toml", "fixed\n")
        self.write(".github/scripts/check-foundation.sh", "reviewed\n")
        self.write("src/engine/src/engine.rs", "old\n")
        self.git("add", "."); self.git("commit", "-qm", "base")
        self.base = self.git("rev-parse", "HEAD").strip()
        self.reviewed = selection.input_fingerprint(selection.tree(self.root, self.base))

    def git(self, *args):
        return subprocess.check_output(["git", *args], cwd=self.root, stderr=subprocess.PIPE, text=True)

    def write(self, path, text):
        target = self.root / path; target.parent.mkdir(parents=True, exist_ok=True); target.write_text(text)

    def commit(self, paths):
        for path in paths: self.write(path, "changed\n")
        self.git("add", "."); self.git("commit", "-qm", "head")
        return self.git("rev-parse", "HEAD").strip()

    def run_selection(self, paths, **kwargs):
        head = self.commit(paths)
        event = {"before": self.base, "after": head, "repository": {"full_name": selection.REPOSITORY}}
        event.update(kwargs.pop("event", {}))
        return selection.select(self.root, kwargs.pop("event_name", "push"), event,
                                kwargs.pop("head", head), kwargs.pop("ref", "refs/heads/main"),
                                fetch=kwargs.pop("fetch", fetcher(evidence(self.base))),
                                reviewed=kwargs.pop("reviewed", self.reviewed))

    def assert_full(self, result):
        self.assertEqual(result["suites"], list(selection.ALL)); self.assertEqual(result["reused"], [])

    def test_engine_changes_keep_real_language_and_async_consumers(self):
        result = self.run_selection(["src/engine/src/engine.rs"])
        self.assertEqual(result["reused"], list(selection.FOUNDATION[:3]))
        for suite in ("foundation-language", "foundation-async", *[s for s in selection.ALL if s not in selection.FOUNDATION]):
            self.assertIn(suite, result["suites"])
        self.assertEqual(result["base"], self.base)
        self.assertIn("evidence", result)

    def test_known_docs_only_can_reuse_all_five(self):
        result = self.run_selection(["docs/validation/release-checklist.md"])
        self.assertEqual(result["reused"], list(selection.FOUNDATION))

    def test_manifest_workflow_foundation_unknown_and_shared_fixture_force_full(self):
        for path in ("Cargo.toml", "Cargo.lock", "src/engine/Cargo.toml", "rust-toolchain.toml",
                     ".cargo/config.toml", ".github/workflows/ci.yml", selection.SELF,
                     "src/foundation/x.rs", "test/foundation/x.rs", "unreviewed.txt",
                     "test/engine/integration/hash_table_compatibility.rs",
                     "docs/validation/nu-ansi-term-examples.md"):
            self.assertIsNone(selection.affected(path), path)
        self.assert_full(self.run_selection([".github/workflows/ci.yml"]))

    def test_pr_dispatch_non_main_and_missing_identity_full(self):
        for name in ("pull_request", "workflow_dispatch", "unknown"):
            self.assert_full(selection.select(self.root, name, {}, HEAD, "refs/heads/main"))
        self.assert_full(self.run_selection(["src/engine/src/engine.rs"], event={"before": "0" * 40}))

    def test_missing_shallow_base_and_checkout_mismatch_full(self):
        self.assert_full(self.run_selection(["src/engine/src/engine.rs"], event={"before": BASE}))

    def test_event_after_repository_and_checkout_mismatch_full(self):
        head = self.commit(["src/engine/src/engine.rs"])
        valid = {"before": self.base, "after": head, "repository": {"full_name": selection.REPOSITORY}}
        for update in ({"after": HEAD}, {"repository": None}, {"repository": []},
                       {"repository": {"full_name": "other/repo"}}, {"before": "unknown"}):
            with self.subTest(update=update):
                event = dict(valid); event.update(update)
                self.assert_full(selection.select(self.root, "push", event, head, "refs/heads/main",
                                                 reviewed=self.reviewed, fetch=fetcher(evidence(self.base))))
        self.assert_full(selection.select(self.root, "push", dict(valid, after=HEAD), HEAD,
                                          "refs/heads/main", reviewed=self.reviewed))
        self.assert_full(selection.select(self.root, "push", valid, head, "refs/heads/other",
                                          reviewed=self.reviewed))

    def test_base_failure_and_malformed_network_result_select_all(self):
        head = self.commit(["src/engine/src/engine.rs"])
        event = {"before": self.base, "after": head, "repository": {"full_name": selection.REPOSITORY}}
        failed = evidence(self.base); failed[1]["jobs"][0]["conclusion"] = "failure"
        for fetch in (fetcher(failed), lambda _: [], lambda _: {"total_count": 101, "workflow_runs": []}):
            self.assert_full(selection.select(self.root, "push", event, head, "refs/heads/main",
                                              fetch=fetch, reviewed=self.reviewed))

    def test_allowlist_is_specific_not_all_source_docs_or_tests(self):
        for path in ("docs/new.md", "src/engine/build.rs", "test/ci/test.py",
                     "src/engine/src/data.json", "tools/history-scale-bench/acceptance-plan.json"):
            self.assertIsNone(selection.affected(path))
        self.assertEqual(selection.affected("tools/history-scale-bench/driver.rs"), {"foundation-language"})
        self.assertEqual(selection.affected("test/server/unit/new.rs"), {"foundation-async"})

    def test_failed_or_unavailable_base_evidence_full(self):
        def unavailable(_): raise OSError("rate limited or unavailable")
        self.assert_full(self.run_selection(["src/engine/src/engine.rs"], fetch=unavailable))

    def test_truncated_or_malformed_http_stream_falls_back_to_full(self):
        head = self.commit(["src/engine/src/engine.rs"])
        event = {"before": self.base, "after": head, "repository": {"full_name": selection.REPOSITORY}}
        for error in (http.client.IncompleteRead(b"partial"), http.client.BadStatusLine("malformed")):
            def unavailable(_, failure=error): raise failure
            self.assert_full(selection.select(self.root, "push", event, head, "refs/heads/main",
                                              fetch=unavailable, reviewed=self.reviewed))

    def test_reviewed_input_fingerprint_change_full(self):
        self.assert_full(self.run_selection(["src/engine/src/engine.rs"], reviewed="0" * 64))

    def test_later_new_consumer_cannot_inherit_old_allowlist(self):
        self.write(".github/scripts/check-foundation.sh", "new engine consumer\n")
        self.git("add", "."); self.git("commit", "-qm", "new consumer")
        self.base = self.git("rev-parse", "HEAD").strip()
        self.assert_full(self.run_selection(["src/engine/src/engine.rs"]))

    def test_non_regular_paths_are_never_reused(self):
        self.git("rm", "-q", "src/engine/src/engine.rs")
        (self.root / "src/engine/src").mkdir(parents=True, exist_ok=True)
        (self.root / "src/engine/src/engine.rs").symlink_to("../../../Cargo.toml")
        self.git("add", "."); self.git("commit", "-qm", "symlink")
        head = self.git("rev-parse", "HEAD").strip()
        result = selection.select(self.root, "push", {"before": self.base, "after": head,
                                  "repository": {"full_name": selection.REPOSITORY}}, head, "refs/heads/main",
                                  reviewed=self.reviewed, fetch=fetcher(evidence(self.base)))
        self.assert_full(result)

    def test_deleted_and_added_paths_are_both_visible_without_rename_detection(self):
        (self.root / "docs").mkdir()
        self.git("mv", "Cargo.toml", "docs/release.md")
        self.git("commit", "-qm", "rename")
        head = self.git("rev-parse", "HEAD").strip()
        result = selection.select(self.root, "push", {"before": self.base, "after": head,
                                  "repository": {"full_name": selection.REPOSITORY}}, head, "refs/heads/main",
                                  reviewed=self.reviewed, fetch=fetcher(evidence(self.base)))
        self.assert_full(result)
        self.assertIn("Cargo.toml", result["changed_paths"])

    def test_workflow_keeps_all_other_jobs_and_no_new_permissions(self):
        source = (ROOT / ".github/workflows/ci.yml").read_text()
        self.assertIn("suite: ${{ fromJSON(needs.static.outputs.suites) }}", source)
        self.assertIn("fetch-depth: 2", source)
        self.assertIn("permissions:\n  contents: read\n", source)
        self.assertNotIn("actions: read", source)
        self.assertIn("python3 .github/scripts/select-ci-suites.py", source)
        release = (ROOT / ".github/workflows/release.yml").read_text()
        self.assertIn("suite: [" + ", ".join(selection.ALL) + "]", release)
        for job in ("web-production:", "browser-smoke:", "smb-sdk:"):
            self.assertIn(job, source)


if __name__ == "__main__":
    unittest.main()
