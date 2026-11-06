"""Host probe checks and exact runner wiring; not Chrome acceptance."""
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]
PROBE = ROOT / "test/tools/browser-smoke"


class PhaseOneFileProbes(unittest.TestCase):
    def test_observer_models_and_negative_publication_examples(self):
        subprocess.run(["node", str(PROBE / "unit/phase1_files_probe_test.mjs")],
                       cwd=ROOT, check=True)

    def test_default_runner_uses_new_checks_without_weakening_old_flows(self):
        main = (PROBE / "runner/main.rs").read_text()
        self.assertIn("phase1_files::disk_read(browser)?;", main)
        self.assertLess(main.index("history_windows::run(browser)?;"),
                        main.index("phase1_files::tags(browser)?;"))
        self.assertLess(main.index("phase1_files::tags(browser)?;"),
                        main.index("session_logout::run(browser, password)?;"))
        for name in ["phase1_disk_read.js", "phase1_tags.js"]:
            self.assertNotIn("new Response(", (PROBE / "runner" / name).read_text())
        self.assertIn("assert_metadata_only(browser)?;",
                      (PROBE / "runner/phase1_files.rs").read_text())


if __name__ == "__main__":
    unittest.main()
