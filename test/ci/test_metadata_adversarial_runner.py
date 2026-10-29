"""Inventory and libtest output admission for the metadata adversarial runner."""
import importlib.util
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "metadata_adversarial_runner", ROOT / "test/engine/unit/metadata_adversarial/run.py"
)
RUNNER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RUNNER)


def summary(passed=1, failed=0, ignored=0, measured=0, filtered=975):
    status = "FAILED" if failed else "ok"
    return (f"test result: {status}. {passed} passed; {failed} failed; {ignored} ignored; "
            f"{measured} measured; {filtered} filtered out; finished in 0.01s\n")


class MetadataAdversarialRunnerTests(unittest.TestCase):
    def setUp(self):
        self.expected = RUNNER.EXPECTED_TESTS.read_text().splitlines()
        self.listing = "\n".join(f"{name}: test" for name in self.expected)

    def test_fixed_inventory_has_36_unique_cases(self):
        self.assertEqual(len(self.expected), 36)
        self.assertEqual(self.expected, sorted(set(self.expected)))

    def test_complete_inventory_accepts_other_engine_tests_and_exact_selection(self):
        listing = self.listing + "\nother_engine_test: test\n"
        self.assertEqual(RUNNER.selected_tests(listing, self.expected, None), self.expected)
        self.assertEqual(RUNNER.selected_tests(listing, self.expected, self.expected[0]),
                         [self.expected[0]])

    def test_missing_inventory_fails_even_when_only_selects_an_existing_case(self):
        listing = "\n".join(f"{name}: test" for name in self.expected[1:])
        for discovered in [listing, ""]:
            with self.subTest(discovered=discovered), self.assertRaisesRegex(RuntimeError, "inventory differs"):
                RUNNER.selected_tests(discovered, self.expected, self.expected[1])

    def test_unexpected_and_duplicate_cases_fail(self):
        for name in ["metadata_adversarial::unexpected", self.expected[0]]:
            with self.subTest(name=name), self.assertRaisesRegex(RuntimeError, "inventory differs"):
                RUNNER.selected_tests(self.listing + f"\n{name}: test", self.expected, None)

    def test_unknown_exact_selection_fails(self):
        with self.assertRaisesRegex(RuntimeError, "requested test"):
            RUNNER.selected_tests(self.listing, self.expected, "not_a_test")

    def test_one_pass_accepts_expected_filtered_cases_and_nocapture_output(self):
        lines = ["test fixture ... diagnostic evidence\n", "ok\n", summary(), "\n"]
        self.assertTrue(RUNNER.passed_exact_execution(0, lines))
        self.assertTrue(RUNNER.passed_exact_execution(0, [summary(filtered=0)]))

    def test_exit_zero_without_one_executed_body_fails(self):
        for line in [summary(passed=0), summary(passed=0, ignored=1),
                     summary(passed=2), summary(ignored=1), summary(measured=1),
                     summary(failed=1)]:
            with self.subTest(line=line):
                self.assertFalse(RUNNER.passed_exact_execution(0, [line]))

    def test_failure_exit_missing_or_ambiguous_summary_fails(self):
        for returncode, lines in [(101, [summary()]), (-9, [summary()]),
                                  (0, []), (0, [summary(), summary()]),
                                  (0, ["test result: ok. truncated\n"])]:
            with self.subTest(returncode=returncode, lines=lines):
                self.assertFalse(RUNNER.passed_exact_execution(returncode, lines))


if __name__ == "__main__":
    unittest.main()
