"""Actual helper handlers with transport/DOM doubles, not real browser testing."""
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]


class HelperForeignConsoleTests(unittest.TestCase):
    def test_complete_database_and_identity_proposal_handlers(self):
        subprocess.run(["node", "test/helper/foreign_chain_console.mjs"],
                       cwd=ROOT, check=True, timeout=30)


if __name__ == "__main__":
    unittest.main()
