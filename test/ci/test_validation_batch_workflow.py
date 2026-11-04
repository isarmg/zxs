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
            'cargo fmt -p platform-io -p storage-engine',
            'cargo clippy --locked --offline -p platform-io -p storage-engine',
            'cargo test --locked --offline -p platform-io -p storage-engine',
            '-p storage-client -p storage-contract --lib',
            '-p storage-engine --test branch_alias_operations',
            '-p storage-server --test http_engine',
            '-p storage-server --no-default-features --all-targets',
            '-p contract-check -p browser-smoke --all-targets -- -D warnings',
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

    def test_independent_wasm_build_overlaps_static_without_bypassing_it(self):
        parts = re.split(r'^  ([a-z][a-z-]+):$',
                         self.batch.split('\njobs:\n', 1)[1], flags=re.MULTILINE)
        jobs = dict(zip(parts[1::2], parts[2::2]))
        # A build can start early, but no browser evidence may bypass either
        # source governance or the exact-run production assets and CSR fixture.
        self.assertNotRegex(jobs['web-production'], r'(?m)^    (?:needs|if):')
        self.assertIn('bash .github/scripts/check.sh static', jobs['static'])
        self.assertIn('bash .github/scripts/check.sh web-production', jobs['web-production'])
        self.assertIn('    needs: [static, web-production]\n', jobs['browser-smoke'])
        self.assertNotRegex(jobs['browser-smoke'], r'(?m)^    if:')

    def test_reuse_is_bound_to_one_parent_non_web_source_and_exact_paths(self):
        script = textwrap.dedent(self.batch.split('        id: scope\n        run: |\n', 1)[1]
                                 .split('      - name:', 1)[0])
        base = '72e7258be1d1a5d461476c259db5ec0eeec7e75f'
        expected_parent = '99b9628d9dca5373f2cd377bd10eced55106a511'
        intermediate = 'd2716dc456c093eaf96d0392a2e0e34304370935'
        prior = '90410707850443016397d9022251c97594abb607'
        expected_chain = f'{expected_parent} {intermediate}\n{intermediate} {prior}\n{prior} {base}'
        self.assertIn(f'base={base}', script)
        self.assertIn(f'expected_parent={expected_parent}', script)
        self.assertIn(f'intermediate={intermediate}', script)
        self.assertIn(f'prior={prior}', script)
        self.assertIn('git rev-list --parents --max-count=3 "$expected_parent"', script)
        self.assertIn('https://github.com/isarmg/zxs/actions/runs/41839207213', script)
        self.assertIn('git show -s --format=%P HEAD', script)
        self.assertIn('git diff --quiet "$base" HEAD -- src \':(exclude)src/web\'', script)
        self.assertIn('git diff --quiet "$base" HEAD -- test ', script)
        self.assertIn('git diff --name-only --no-renames "$base" HEAD', script)
        self.assertNotIn(':(exclude)test/engine', script)
        self.assertNotIn(':(exclude)test/server', script)
        paths = [
            '.github/workflows/validate-ui-smb-history.yml',
            'compliance/external-tests/integrated.json', 'compliance/rust-sources.json',
            'docs/THIRD_PARTY_RUST_SOURCES.md', 'docs/components/interface/src/components.md',
            'docs/components/web/public/styles.md', 'docs/tools/browser-smoke.md',
            'src/web/public/styles/components/auth.css', 'src/web/src/setup.rs',
            'test/ci/test_validation_batch_workflow.py',
            'test/interface/integration/style_contracts.rs',
            'test/tools/browser-smoke/runner/auth_layout.js',
            'test/tools/browser-smoke/runner/auth_layout.rs',
            'test/tools/browser-smoke/runner/management.rs',
            'test/tools/browser-smoke/runner/file_row_pixels.js',
            'test/tools/browser-smoke/unit/auth_layout_probe_test.mjs',
            'test/tools/browser-smoke/unit/file_row_pixels_probe_test.mjs',
            'test/tools/browser-smoke/unit/setup_lifecycle_probe_test.mjs',
        ]
        # Exercise the actual script with exact command replies in GitHub static.
        # No changed dependency, native test, workflow or unrelated Web path may reuse.
        git = r'''git() {
            case "$*" in
                "show -s --format=%P HEAD")
                    [ "$TEST_GIT_FAIL" != parent ] || return 42
                    printf '%s\n' "$TEST_PARENT" ;;
                "rev-list --parents --max-count=3 $expected_parent")
                    [ "$TEST_GIT_FAIL" != ancestor ] || return 42
                    printf '%s\n' "$TEST_ANCESTOR" ;;
                "diff --quiet $base HEAD -- src :(exclude)src/web")
                    [ "$TEST_GIT_FAIL" != source ] || return 42
                    return "$TEST_SOURCE_CHANGED" ;;
                "diff --quiet $base HEAD -- test :(exclude)test/interface :(exclude)test/tools/browser-smoke :(exclude)test/ci/test_validation_batch_workflow.py")
                    [ "$TEST_GIT_FAIL" != tests ] || return 42
                    return "$TEST_TESTS_CHANGED" ;;
                "diff --name-only --no-renames $base HEAD")
                    [ "$TEST_GIT_FAIL" != paths ] || return 42
                    printf '%s\n' "$TEST_PATHS" ;;
                *) return 99 ;;
            esac
        }
        '''
        cases = [(expected_parent, expected_chain, paths, 0, 0, True, '')]
        for parent in ['another-parent', expected_parent + ' second-parent', '']:
            cases.append((parent, expected_chain, paths, 0, 0, False, ''))
        for ancestor in ['another-base', expected_chain + ' second-parent', '',
                         f'{expected_parent} {intermediate}\n{intermediate} {prior}\n{prior} wrong-base',
                         f'{expected_parent} {intermediate}\n{intermediate} wrong-prior\n{prior} {base}',
                         f'{expected_parent} wrong-parent\n{intermediate} {prior}\n{prior} {base}']:
            cases.append((expected_parent, ancestor, paths, 0, 0, False, ''))
        cases.extend([(expected_parent, expected_chain, paths, 1, 0, False, ''),
                      (expected_parent, expected_chain, paths, 0, 1, False, '')])
        for extra in ['src/smb/src/client.rs', 'src/platform/src/payload.rs',
                      'src/web/Cargo.toml', 'src/web/src/unknown.rs',
                      'test/engine/unit/tests.rs', 'test/server/unit/tests.rs',
                      'test/platform/unit/payload/tests.rs', 'test/tools/contract-check/main.rs',
                      '.github/workflows/ci.yml', '.github/workflows/smb-sdk.yml',
                      'Cargo.lock', 'Cargo.toml', '.cargo/config.toml', 'rust-toolchain.toml',
                      'docs/contracts/core.schema.json', 'tools/rust-sources/manage.py',
                      'docs/unrelated.md', 'test/tools/browser-smoke/runner/unknown.rs']:
            cases.append((expected_parent, expected_chain, paths + [extra], 0, 0, False, ''))
        for failed in ['parent', 'ancestor', 'source', 'tests', 'paths']:
            cases.append((expected_parent, expected_chain, paths, 0, 0, False, failed))
        for parent, ancestor, changed, source_changed, tests_changed, expected, failed in cases:
            with self.subTest(parent=parent, ancestor=ancestor, changed=changed, failed=failed,
                              source_changed=source_changed, tests_changed=tests_changed):
                with tempfile.TemporaryDirectory() as directory:
                    output = Path(directory) / 'output'
                    env = dict(os.environ, TEST_PARENT=parent, TEST_ANCESTOR=ancestor, TEST_PATHS='\n'.join(changed),
                               TEST_SOURCE_CHANGED=str(source_changed),
                               TEST_TESTS_CHANGED=str(tests_changed), TEST_GIT_FAIL=failed,
                               GITHUB_OUTPUT=str(output))
                    subprocess.run(['bash', '-euo', 'pipefail', '-c', git + script],
                                   check=True, env=env, capture_output=True, text=True)
                    self.assertEqual(output.read_text(), f'web_only_retry={str(expected).lower()}\n')

    def test_web_retry_keeps_all_changed_checks_and_full_fallback(self):
        condition = "if: needs.static.outputs.web_only_retry != 'true'"
        self.assertEqual(self.batch.count(condition), 2)
        self.assertIn(f'  rust-http:\n    {condition}', self.batch)
        self.assertIn(f'  smb-sdk:\n    {condition}', self.batch)
        self.assertNotIn('test_only_retry', self.batch)
        for marker in ['Check affected native targets and no-core server',
                       'Exercise engine, server, client and wire unit contracts',
                       'Exercise history integration', 'Exercise real HTTP routes',
                       'Check changed wire contracts and SMB feature combinations',
                       'Build production Web distribution',
                       'Exercise real login, setup, monitor, file rows and history flows']:
            self.assertNotIn('if:', step_body(self.batch, marker))
        parts = re.split(r'^  ([a-z][a-z-]+):$',
                         self.batch.split('\njobs:\n', 1)[1], flags=re.MULTILINE)
        jobs = dict(zip(parts[1::2], parts[2::2]))
        for name in ['static', 'web-tests', 'web-production', 'browser-smoke']:
            self.assertNotRegex(jobs[name], r'(?m)^    if:')
        browser_lint = step_body(self.batch, 'Lint the changed browser validation target')
        self.assertIn("if: needs.static.outputs.web_only_retry == 'true'", browser_lint)
        self.assertIn('cargo fmt -p browser-smoke -- --check', browser_lint)
        self.assertIn('cargo clippy --locked --offline -p browser-smoke --all-targets -- -D warnings', browser_lint)

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
        self.assertIn('            artifacts/browser-smoke/placement/\n', self.batch)
        for action in re.findall(r'uses: (actions/[^\s]+)', self.batch):
            self.assertRegex(action, r'^actions/[a-z-]+@[0-9a-f]{40}$')


if __name__ == '__main__':
    unittest.main()
