"""Source contracts for one temporary batch, not evidence that Actions passed."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
BRANCH = 'validation/ui-smb-history-20261104'


class ValidationBatchWorkflow(unittest.TestCase):
    def setUp(self):
        self.normal = (ROOT / '.github/workflows/ci.yml').read_text()
        self.batch = (ROOT / '.github/workflows/validate-ui-smb-history.yml').read_text()

    def test_only_one_branch_uses_the_batch_and_tags_keep_normal_ci(self):
        normal_trigger = self.normal.split('\npermissions:', 1)[0]
        batch_trigger = self.batch.split('\npermissions:', 1)[0]
        self.assertIn(f'    branches-ignore: [{BRANCH}]', normal_trigger)
        self.assertIn("    tags: ['**']", normal_trigger)
        self.assertIn('  pull_request:\n  workflow_dispatch:', normal_trigger)
        self.assertIn(f'    branches: [{BRANCH}]', batch_trigger)
        for other in ['pull_request:', 'workflow_dispatch:', 'tags:', 'schedule:']:
            self.assertNotIn(other, batch_trigger)
        self.assertNotIn('branches-ignore:', batch_trigger)

    def test_batch_has_affected_native_web_browser_and_real_samba_jobs(self):
        jobs = set(re.findall(r'^  ([a-z][a-z-]+):$',
                              self.batch.split('\njobs:\n', 1)[1], re.MULTILINE))
        self.assertEqual(jobs, {'static', 'rust-http', 'web-tests',
                                'web-production', 'browser-smoke', 'smb-sdk'})
        for command in [
            'bash .github/scripts/check.sh static',
            '-p storage-client -p storage-contract --lib',
            '-p storage-engine --test branch_alias_operations',
            '-p storage-server --test http_engine',
            '-p storage-server --no-default-features --all-targets',
            '-p web-app -p web-interface',
            'bash .github/scripts/check.sh contracts',
            'bash .github/scripts/check.sh smb-protocol',
            'python3 .github/scripts/check-smb-budget-io.py',
            'bash .github/scripts/check.sh web-production',
            'bash .github/scripts/check.sh browser-smoke',
            'uses: ./.github/workflows/smb-sdk.yml',
        ]:
            self.assertIn(command, self.batch)
        for broad in ['check.sh workspace', 'check.sh security', 'check-foundation.sh',
                      'release-publish.sh', 'release-assemble.sh', 'select-ci-suites.py']:
            self.assertNotIn(broad, self.batch)

    def test_batch_keeps_read_permissions_and_same_run_assets(self):
        self.assertIn('permissions:\n  contents: read\n', self.batch)
        self.assertNotIn('contents: write', self.batch)
        self.assertNotIn('secrets:', self.batch)
        self.assertNotIn('continue-on-error:', self.batch)
        self.assertNotIn('--no-sandbox', self.batch)
        self.assertNotIn('ref:', self.batch)
        self.assertNotIn('run-id:', self.batch)
        self.assertNotIn('github-token:', self.batch)
        self.assertIn('name: web-dist', self.batch)
        self.assertIn('name: csr-template-fixture', self.batch)
        for action in re.findall(r'uses: (actions/[^\s]+)', self.batch):
            self.assertRegex(action, r'^actions/[a-z-]+@[0-9a-f]{40}$')


if __name__ == '__main__':
    unittest.main()
