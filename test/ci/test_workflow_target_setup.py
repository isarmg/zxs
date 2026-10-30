"""Keep cross-target checks paired with their runner prerequisites."""
from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[2]


class WorkflowTargetSetupTests(unittest.TestCase):
    def test_cross_target_suites_install_wasm_before_running(self):
        workflow = (ROOT / ".github/workflows/ci.yml").read_text()
        matrix_job = workflow.split("\n  tests:\n", 1)[1].split(
            "\n  web-production:\n", 1
        )[0]
        setup = re.search(
            r"- name: Install WASM standard library[^\n]*\n"
            r"\s+if: ([^\n]+)\n"
            r"\s+run: rustup target add wasm32-unknown-unknown",
            matrix_job,
        )
        self.assertIsNotNone(setup, "matrix jobs need explicit WASM target setup")
        self.assertEqual(
            setup[1].strip(),
            "env.ZXS_RUN_SOURCE_VALIDATION == 'true' && (matrix.suite == 'quality' || matrix.suite == 'build-tools' || startsWith(matrix.suite, 'foundation-'))",
        )
        self.assertLess(setup.end(), matrix_job.index("Run locked test suite"))


if __name__ == "__main__":
    unittest.main()
