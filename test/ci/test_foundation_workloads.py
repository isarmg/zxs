"""CI/release must fan out the complete foundation inventory exactly once."""
import json
import importlib.util
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
SHARDS = {'foundation-platform', 'foundation-crypto', 'foundation-data',
          'foundation-language', 'foundation-async'}
PORTABLE = {'rust-sources', 'quality', 'security', 'workspace', 'web',
            'contracts', 'build-tools', 'smb-protocol'}


def job(source, name):
    bodies = re.split(r'^  ([A-Za-z0-9_-]+):\s*$', source.split('\njobs:\n', 1)[1], flags=re.MULTILINE)
    return dict(zip(bodies[1::2], bodies[2::2]))[name]


class FoundationWorkloads(unittest.TestCase):
    def test_inventory_has_exact_nonempty_functional_shards(self):
        inventory = json.loads((ROOT / 'test/ci/foundation-commands.json').read_text())
        self.assertEqual(set(inventory), SHARDS)
        self.assertTrue(all(inventory.values()))

    def test_ci_and_release_matrices_cover_every_suite_once(self):
        for workflow, name in [('ci.yml', 'tests'), ('release.yml', 'verify')]:
            with self.subTest(workflow=workflow):
                body = job((ROOT / '.github/workflows' / workflow).read_text(), name)
                if workflow == 'ci.yml':
                    self.assertIn('suite: ${{ fromJSON(needs.static.outputs.suites) }}', body)
                    self.assertIn('needs: static', body)
                    source = (ROOT / '.github/workflows' / workflow).read_text()
                    static = job(source, 'static')
                    self.assertIn('suites: ${{ steps.suites.outputs.suites }}', static)
                    self.assertIn('id: suites\n        run: python3 .github/scripts/select-ci-suites.py', static)
                    spec = importlib.util.spec_from_file_location(
                        'ci_suite_inventory', ROOT / '.github/scripts/select-ci-suites.py')
                    selector = importlib.util.module_from_spec(spec)
                    spec.loader.exec_module(selector)
                    suites = list(selector.ALL)
                    fallback = selector.select(ROOT, 'workflow_dispatch', {}, '', '')
                    self.assertEqual(fallback['suites'], suites)
                    self.assertEqual(fallback['reused'], [])
                else:
                    match = re.search(r'^\s+suite: \[([^\]]+)\]$', body, re.MULTILINE)
                    self.assertIsNotNone(match)
                    suites = [item.strip() for item in match[1].split(',')]
                self.assertEqual(set(suites), SHARDS | PORTABLE)
                self.assertEqual(len(suites), len(set(suites)))
                self.assertNotIn('foundation', suites, 'aggregate would duplicate every shard')
                self.assertIn('fail-fast: false', body)
                self.assertIn('bash .github/scripts/check.sh "${{ matrix.suite }}"', body)
                self.assertNotIn('for suite in ', body)

    def test_system_sqlite_sdk_and_probe_run_only_in_data_cells(self):
        for workflow, name in [('ci.yml', 'tests'), ('release.yml', 'verify')]:
            with self.subTest(workflow=workflow):
                body = job((ROOT / '.github/workflows' / workflow).read_text(), name)
                setup = re.search(r'- name: Install system SQLite SDK[^\n]*\n\s+if: ([^\n]+)\n\s+run: \|\n([^\n]+)\n([^\n]+)', body)
                probe = re.search(r'- name: Verify isolated system SQLite consumer\n\s+if: ([^\n]+)\n\s+run: ([^\n]+)', body)
                self.assertIsNotNone(setup)
                self.assertIsNotNone(probe)
                self.assertEqual(setup[1], "matrix.suite == 'foundation-data'")
                self.assertEqual(probe[1], setup[1])
                self.assertIn('libsqlite3-dev clang libclang-dev pkg-config', setup[3])
                self.assertEqual(probe[2], 'python3 .github/scripts/check-sqlite-system.py')
                self.assertLess(setup.start(), body.index('bash .github/scripts/check.sh'))
                self.assertLess(body.index('bash .github/scripts/check.sh'), probe.start())

    def test_valgrind_sdk_and_gate_are_required_in_crypto_cells(self):
        inventory = json.loads((ROOT / 'test/ci/foundation-commands.json').read_text())
        gate = {'tool': 'python3', 'args': ['.github/scripts/check-valgrind-pki.py']}
        self.assertEqual(inventory['foundation-crypto'].count(gate), 1)
        for workflow, name in [('ci.yml', 'tests'), ('release.yml', 'verify')]:
            body = job((ROOT / '.github/workflows' / workflow).read_text(), name)
            setup = body.split('- name: Install Valgrind SDK and loader debug symbols\n', 1)[1].split('- name:', 1)[0]
            self.assertIn("if: matrix.suite == 'foundation-crypto'", setup)
            self.assertIn('valgrind libc6-dbg clang libclang-dev', setup)
            self.assertLess(body.index('Install Valgrind SDK'), body.index('bash .github/scripts/check.sh'))
            evidence = body.split('- name: Retain PKI Valgrind positive and negative evidence\n', 1)[1].split('- name:', 1)[0]
            self.assertIn("always() && matrix.suite == 'foundation-crypto'", evidence)
            self.assertIn('target/pki-valgrind/logs/', evidence)
            self.assertIn('if-no-files-found: error', evidence)

    def test_release_still_waits_for_all_verification_cells(self):
        source = (ROOT / '.github/workflows/release.yml').read_text()
        self.assertRegex(job(source, 'assemble'), r'needs: \[[^\]]*\bverify\b[^\]]*\]')
        self.assertIn('needs: version', job(source, 'verify'))
        self.assertIn("tags: ['v*']", source)
        self.assertIn('workflow_dispatch:', source)


if __name__ == '__main__':
    unittest.main()
