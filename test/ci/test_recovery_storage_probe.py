"""Recovery observer model checks, not actual browser or storage acceptance."""
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]


class RecoveryStorageProbe(unittest.TestCase):
    def test_naming_observer_checks_originals_and_preserves_other_records(self):
        subprocess.run(
            ['node', str(ROOT / 'test/tools/browser-smoke/unit/recovery_storage_probe_test.mjs')],
            cwd=ROOT, check=True,
        )


if __name__ == '__main__':
    unittest.main()
