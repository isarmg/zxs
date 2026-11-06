"""Host fault-observer and runner wiring checks, not Chrome acceptance."""
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]
PROBE = ROOT / "test/tools/browser-smoke"


class TagUnknownProbe(unittest.TestCase):
    def test_host_observer_positive_and_negative_examples(self):
        subprocess.run(["node", str(PROBE / "unit/tag_unknown_probe_test.mjs")],
                       cwd=ROOT, check=True)

    def test_runner_keeps_existing_checks_and_collects_evidence_before_restore(self):
        main = (PROBE / "runner/main.rs").read_text()
        scene = (PROBE / "runner/tag_unknown.rs").read_text()
        self.assertLess(main.index("phase1_files::tags(browser)?;"),
                        main.index("tag_unknown::run(browser)?;"))
        self.assertLess(main.index("tag_unknown::run(browser)?;"),
                        main.index("session_logout::run(browser, password)?;"))
        self.assertIn("phase1_files::edit_current_tags(browser)?;", scene)
        self.assertIn("assert_metadata_only(browser)", scene)
        self.assertEqual(scene.count("with_cleanup("), 2)
        self.assertLess(scene.index('"tag-unknown-recovery"'), scene.index("?.restore()"))
        self.assertIn("../unit/tag_unknown_tests.rs", scene)


if __name__ == "__main__":
    unittest.main()
