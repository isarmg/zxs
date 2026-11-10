"""First-install handlers with transport/DOM doubles, not real browser testing."""
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]


class HelperFirstInstallTests(unittest.TestCase):
    def test_system_package_installation_and_finish(self):
        subprocess.run(["node", "test/helper/packages_console.mjs"],
                       cwd=ROOT, check=True, timeout=30)

    def test_explicit_service_account_preparation_and_recovery(self):
        subprocess.run(["node", "test/helper/service_account_console.mjs"],
                       cwd=ROOT, check=True, timeout=30)

    def test_unsubmitted_directory_correction_and_resume(self):
        subprocess.run(["node", "test/helper/first_install_roots.mjs"],
                       cwd=ROOT, check=True, timeout=30)

    def test_offline_activation_handoff(self):
        subprocess.run(["node", "test/helper/activation_console.mjs"],
                       cwd=ROOT, check=True, timeout=30)


if __name__ == "__main__":
    unittest.main()
