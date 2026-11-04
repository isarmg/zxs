"""Temporary placement workflow source contracts, not execution evidence."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
BRANCH = 'verify/placement-object-20261104'


class PlacementValidationWorkflow(unittest.TestCase):
    def setUp(self):
        self.workflow = (ROOT / '.github/workflows/validate-placement-object.yml').read_text()
        self.normal = (ROOT / '.github/workflows/ci.yml').read_text()

    def test_exact_branch_only_and_normal_main_pr_tag_gates_retained(self):
        trigger = self.workflow.split('\npermissions:', 1)[0]
        self.assertIn(f'    branches: [{BRANCH}]', trigger)
        for other in ['pull_request:', 'workflow_dispatch:', 'tags:', 'schedule:']:
            self.assertNotIn(other, trigger)
        normal = self.normal.split('\npermissions:', 1)[0]
        self.assertIn('    branches-ignore: [validation/ui-smb-history-20261104, '
                      f'{BRANCH}]', normal)
        self.assertIn("    tags: ['**']", normal)
        self.assertIn('  pull_request:\n  workflow_dispatch:', normal)

    def test_all_thirteen_focused_cases_must_run_without_ignored_failures(self):
        for command in [
            '-p storage-engine --lib placement_configuration',
            '-p storage-server --lib http_placement_configuration',
            '-p storage-server --test http_engine placement_configuration',
            '-p web-app --lib placement_configuration',
        ]:
            self.assertIn(command, self.workflow)
        for count, occurrences in [(5, 1), (1, 2), (6, 1)]:
            self.assertEqual(self.workflow.count(
                f'{count} passed; 0 failed; 0 ignored;'), occurrences)
        self.assertEqual(self.workflow.count('set -euo pipefail'), 4)
        self.assertNotIn('continue-on-error:', self.workflow)

    def test_type_checks_cover_native_no_core_and_release_wasm(self):
        for command in [
            '-p storage-contract -p storage-engine -p storage-server --all-targets -- -D warnings',
            '-p storage-server --no-default-features --all-targets -- -D warnings',
            '-p web-app --all-targets -- -D warnings',
            '-p web-app --target wasm32-unknown-unknown --release -- -D warnings',
            'cargo build --locked --offline -p web-app --target wasm32-unknown-unknown --release',
        ]:
            self.assertIn(command, self.workflow)
        self.assertEqual(self.workflow.count('    needs: static'), 2)
        self.assertIn('test "$(git rev-parse HEAD)" = "$GITHUB_SHA"', self.workflow)

    def test_validation_is_read_only_pinned_and_does_not_claim_browser_acceptance(self):
        self.assertIn('permissions:\n  contents: read\n', self.workflow)
        self.assertEqual(self.workflow.count('persist-credentials: false'), 3)
        self.assertEqual(self.workflow.count('bash .github/scripts/initialize-cargo-home.sh'), 2)
        for forbidden in ['contents: write', 'secrets:', 'ref:', '--no-sandbox',
                          'release-publish.sh', 'check.sh workspace', 'check-foundation.sh',
                          'check.sh browser-smoke', 'check.sh security']:
            self.assertNotIn(forbidden, self.workflow)
        for action in re.findall(r'uses: (actions/[^\s]+)', self.workflow):
            self.assertRegex(action, r'^actions/[a-z-]+@[0-9a-f]{40}$')


if __name__ == '__main__':
    unittest.main()
