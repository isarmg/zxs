"""Source contracts for one temporary batch, not evidence that Actions passed."""
from pathlib import Path
import os
import re
import subprocess
import tempfile
import textwrap
import unittest

ROOT = Path(__file__).resolve().parents[2]
BRANCH = 'validation/ui-smb-history-20261104'


def step_body(workflow, name):
    following = workflow.split(f'- name: {name}', 1)[1]
    return re.split(r'\n(?:      - |  [a-z][a-z-]*:)', following, maxsplit=1)[0]


class ValidationBatchWorkflow(unittest.TestCase):
    def setUp(self):
        self.normal = (ROOT / '.github/workflows/ci.yml').read_text()
        self.batch = (ROOT / '.github/workflows/validate-ui-smb-history.yml').read_text()

    def test_each_temporary_branch_uses_its_own_batch_and_tags_keep_normal_ci(self):
        normal_trigger = self.normal.split('\npermissions:', 1)[0]
        batch_trigger = self.batch.split('\npermissions:', 1)[0]
        self.assertIn(f'    branches-ignore: [{BRANCH}, verify/placement-object-20261104]', normal_trigger)
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

    def test_reuse_is_bound_to_one_parent_source_tree_and_exact_paths(self):
        script = textwrap.dedent(self.batch.split('        id: scope\n        run: |\n', 1)[1]
                                 .split('      - name:', 1)[0])
        base = '01305735ce153015a08bf14c6920cdce40b8b79b'
        parent = '8019ec672d666fb3812af1231be2e766114b1ef3'
        self.assertIn(f'base={base}', script)
        self.assertIn(f'expected_parent={parent}', script)
        self.assertIn('fetch-depth: 3', self.batch)
        self.assertIn('git diff --name-only --no-renames "$base" HEAD', script)
        paths = [
            '.github/workflows/validate-ui-smb-history.yml',
            'docs/tools/browser-smoke.md',
            'test/ci/test_validation_batch_workflow.py',
            'test/ci/test_file_row_decoration.py',
            'test/engine/integration/branch_alias_operations/file_history_merge_scenarios.rs',
            'test/tools/browser-smoke/runner/main.rs',
            'test/tools/browser-smoke/runner/file_row_pointer.js',
            'test/tools/browser-smoke/unit/file_row_pointer_probe_test.mjs',
        ]
        # Execute the actual gate with controlled git replies, not the product.
        # Every unrelated file or mismatched commit/tree must restore coverage.
        git = r'''git() {
            if [ "$*" = "$TEST_GIT_FAIL" ]; then return 42; fi
            case "$*" in
                "rev-parse HEAD^") printf '%s\n' "$TEST_PARENT" ;;
                "rev-parse $expected_parent^") printf '%s\n' "$base" ;;
                "rev-parse HEAD:src") printf '%s\n' "$TEST_SOURCE" ;;
                "rev-parse $base:src") printf '%s\n' baseline-source ;;
                "diff --name-only --no-renames $base HEAD") printf '%s\n' "$TEST_PATHS" ;;
                *) return 99 ;;
            esac
        }
        '''
        cases = [
            (parent, 'baseline-source', paths, True, ''),
            ('another-parent', 'baseline-source', paths, False, ''),
            (parent, 'changed-source', paths, False, ''),
        ]
        for extra in ['src/smb/src/client.rs', 'test/smb/unit/client/tests.rs',
                      'test/interface/integration/style_contracts.rs',
                      '.github/workflows/smb-sdk.yml', 'Cargo.lock',
                      'docs/unrelated.md', 'test/tools/browser-smoke/runner/unknown.rs']:
            cases.append((parent, 'baseline-source', paths + [extra], False, ''))
        for failed in ['rev-parse HEAD^', f'rev-parse {parent}^', 'rev-parse HEAD:src', f'rev-parse {base}:src',
                       f'diff --name-only --no-renames {base} HEAD']:
            cases.append((parent, 'baseline-source', paths, False, failed))
        for parent, source, changed, expected, failed in cases:
            with self.subTest(parent=parent, source=source, changed=changed, failed=failed):
                with tempfile.TemporaryDirectory() as directory:
                    output = Path(directory) / 'output'
                    env = dict(os.environ, TEST_PARENT=parent, TEST_SOURCE=source,
                               TEST_PATHS='\n'.join(changed), TEST_GIT_FAIL=failed,
                               GITHUB_OUTPUT=str(output))
                    subprocess.run(['bash', '-euo', 'pipefail', '-c', git + script],
                                   check=True, env=env, capture_output=True, text=True)
                    self.assertEqual(output.read_text(), f'test_only_retry={str(expected).lower()}\n')

    def test_retry_preserves_unrun_http_protocol_and_exact_head_browser(self):
        condition = "if: needs.static.outputs.test_only_retry != 'true'"
        self.assertEqual(self.batch.count(condition), 4)
        self.assertIn("- name: Lint the changed history integration target\n"
                      "        if: needs.static.outputs.test_only_retry == 'true'\n"
                      "        run: cargo clippy --locked --offline -p storage-engine "
                      "--test branch_alias_operations -- -D warnings", self.batch)
        for marker in ['Check affected native targets and no-core server',
                       'Exercise engine, server, client and wire unit contracts']:
            self.assertIn(f'- name: {marker}\n        {condition}', self.batch)
        self.assertIn(f'name: Affected Web state and style contracts\n    {condition}', self.batch)
        self.assertIn(f'  smb-sdk:\n    {condition}', self.batch)
        for marker in ['Exercise history integration and real HTTP routes',
                       'Check changed wire contracts and SMB feature combinations',
                       'Build production Web distribution',
                       'Exercise real login, setup, monitor, file rows and history flows']:
            step = step_body(self.batch, marker)
            self.assertNotIn(condition, step)

    def test_step_scope_stops_at_next_action_or_job(self):
        selected = "      - name: selected\n        run: keep-this\n"
        for following in ["      - name: next\n        if: unrelated\n",
                          "      - uses: actions/next\n        if: unrelated\n",
                          "  web-tests:\n    if: unrelated\n"]:
            self.assertEqual(step_body(selected + following, 'selected'),
                             "\n        run: keep-this")

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
