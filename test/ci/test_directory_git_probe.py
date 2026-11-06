"""Fetch observer and runner wiring checks, not actual browser acceptance."""
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[2]
PROBE = ROOT / 'test/tools/browser-smoke'


class DirectoryGitProbe(unittest.TestCase):
    def test_observer_retains_real_responses_and_rejects_retired_requests(self):
        subprocess.run(['node', str(PROBE / 'unit/directory_git_probe_test.mjs')],
                       cwd=ROOT, check=True)

    def test_parent_child_flow_and_persistent_fixture_are_in_the_default_runner(self):
        main = (PROBE / 'runner/main.rs').read_text()
        self.assertIn('directory_git::children(browser)?;', main)
        self.assertIn('directory_git::retired_request(url, origin)', main)
        fixture = (PROBE / 'runner/fixture.rs').read_text()
        self.assertIn('super::directory_git::fixture::seed(&mut engine)?;', fixture)
        children = (PROBE / 'runner/directory_git/children.rs').read_text()
        self.assertIn('window.__zxsDirectoryGit.writes.length===7', children)
        self.assertIn('window.__zxsDirectoryGit.restore()', children)
        self.assertNotIn('new Response(', children)


if __name__ == '__main__':
    unittest.main()
