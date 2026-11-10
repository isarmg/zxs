"""Retained recipe and restored manual-CI wiring invariants; not run evidence."""
import copy
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import tomllib
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
MAP = json.loads((ROOT / "test/ci/fixtures/foundation-test-coverage.json").read_text())
CATALOG = json.loads((ROOT / "compliance/rust-sources.json").read_text())
WORKSPACES = json.loads((ROOT / "tools/rust-sources/workspaces.json").read_text())
PRODUCTION = {profile["id"] for profile in WORKSPACES["profiles"] if profile["production"]}
SPEC = importlib.util.spec_from_file_location("behaviour_gate", ROOT / ".github/scripts/check-foundation-behaviour.py")
BEHAVIOUR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BEHAVIOUR)
COVERAGE_SPEC = importlib.util.spec_from_file_location(
    "foundation_coverage", ROOT / "tools/rust-sources/foundation_coverage.py")
COVERAGE = importlib.util.module_from_spec(COVERAGE_SPEC)
COVERAGE_SPEC.loader.exec_module(COVERAGE)


class FoundationCoverageConsumerTests(unittest.TestCase):
    def test_cargo_tree_disables_forced_ci_color_at_process_boundary(self):
        profile = {
            "id": "sqlite-pool-validation", "manifests": ["src/platform/Cargo.toml"],
            "targets": ["x86_64-unknown-linux-gnu"], "features": ["platform-io/sqlite-pool"],
        }
        directory = "src/foundation/code-generation/proc-macro2"
        dependency = f"proc-macro2 v1.0.107 ({ROOT / directory})|default,proc-macro"
        output = (f"platform-io v0.1.0 ({ROOT / 'src/platform'})|sqlite-pool\n"
                  f"{dependency}\n{dependency} (*)\n")
        result = subprocess.CompletedProcess([], 0, output, "")
        with patch.dict(os.environ, {"CARGO_TERM_COLOR": "always"}), \
                patch.object(COVERAGE.subprocess, "run", return_value=result) as run:
            actual = COVERAGE.consumers([profile])
            self.assertEqual(os.environ["CARGO_TERM_COLOR"], "always")
        run.assert_called_once_with(
            ["cargo", "tree", "--locked", "--offline", "--color", "never",
             "-p", "platform-io", "--target", "x86_64-unknown-linux-gnu",
             "-e", "normal,build", "--prefix", "none", "-f", "{p}|{f}",
             "--features", "platform-io/sqlite-pool"],
            cwd=ROOT, text=True, capture_output=True, check=True,
        )
        self.assertEqual(actual, {directory: [{
            "profile": "sqlite-pool-validation", "target": "x86_64-unknown-linux-gnu",
            "features": ["default", "proc-macro"], "consumers": ["platform-io"],
        }]})


class FoundationCoverageReachabilityTests(unittest.TestCase):
    def test_retained_routes_match_restored_full_ci_without_claiming_execution(self):
        spec = importlib.util.spec_from_file_location(
            'development_ci_coverage', ROOT / '.github/scripts/development-ci.py')
        policy = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(policy)
        policy.check_workflows(ROOT)
        current = policy.receipt()
        self.assertEqual(current['mode'], 'manual-full-ci')
        self.assertEqual(current['paused_suites'], [])
        self.assertEqual(current['paused_jobs']['ci.yml'], [])
        self.assertEqual(current['paused_jobs']['smb-sdk.yml'], [])
        declared = COVERAGE.retained_recipe_scripts()
        for shard in ('workspace', 'foundation-crypto', 'foundation-data',
                      'foundation-language', 'foundation-async'):
            self.assertIn(shard, declared)
            self.assertIn(shard, current['suites'])
        self.assertIn('browser-smoke', declared)
        self.assertFalse(COVERAGE.SCOPE['execution_evidence'])
        self.assertIn('publish', current['paused_jobs']['release.yml'])
        self.assertEqual(current['reused'], [])

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='foundation-wiring-')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        for relative in ('.github/ci/full-workflows/ci.yml', '.github/scripts/check.sh',
                         '.github/scripts/check-foundation.sh', '.github/scripts/select-ci-suites.py'):
            destination = self.root / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_text((ROOT / relative).read_text())
        self.script = '.github/scripts/check-foundation-behaviour.py'
        (self.root / self.script).touch()
        self.gate = {'shard': 'foundation-async', 'scripts': [self.script]}

    def edit(self, relative, old, new):
        path = self.root / relative
        source = path.read_text()
        self.assertIn(old, source)
        path.write_text(source.replace(old, new))

    def validate(self):
        with patch.object(COVERAGE, 'ROOT', self.root):
            COVERAGE.validate_gate(self.gate, COVERAGE.retained_recipe_scripts())

    def test_all_222_packages_and_generated_behaviour_have_retained_recipe_routes(self):
        self.assertEqual(len(MAP['packages']), 222)
        reachable = COVERAGE.retained_recipe_scripts()
        registry = json.loads((ROOT / 'tools/rust-sources/foundation-test-gates.json').read_text())
        for gate in registry['entries']:
            COVERAGE.validate_gate(gate, reachable)
        for package in MAP['packages']:
            for gate in package['gates']:
                COVERAGE.validate_gate(gate, reachable)
        for entries in COVERAGE.behaviour_entries(BEHAVIOUR).values():
            for gate in entries:
                COVERAGE.validate_gate(gate, reachable)

    def test_comment_only_is_not_a_command(self):
        self.edit('.github/scripts/check-foundation.sh',
                  '    python3 ' + self.script, '    # python3 ' + self.script)
        with self.assertRaisesRegex(AssertionError, 'not invoked'):
            self.validate()

    def test_orphan_script_cannot_supply_a_route(self):
        self.edit('.github/scripts/check-foundation.sh', '    python3 ' + self.script, '')
        (self.root / '.github/scripts/orphan.sh').write_text('python3 ' + self.script + '\n')
        with self.assertRaises(AssertionError):
            self.validate()

    def test_same_basename_different_path_is_not_the_entrypoint(self):
        self.edit('.github/scripts/check-foundation.sh',
                  'python3 ' + self.script, 'python3 elsewhere/check-foundation-behaviour.py')
        with self.assertRaisesRegex(AssertionError, 'not invoked'):
            self.validate()

    def test_wrong_shard_is_rejected(self):
        self.gate['shard'] = 'foundation-data'
        with self.assertRaisesRegex(AssertionError, 'not invoked'):
            self.validate()

    def test_command_removed_from_workflow_cannot_be_replaced_by_comment(self):
        self.edit('.github/ci/full-workflows/ci.yml',
                  '        run: bash .github/scripts/check.sh "${{ matrix.suite }}"',
                  '        # run: bash .github/scripts/check.sh "${{ matrix.suite }}"')
        with self.assertRaises(AssertionError):
            self.validate()

    def test_unreachable_conditional_shell_command_fails_explicitly(self):
        self.edit('.github/scripts/check-foundation.sh', '    python3 ' + self.script,
                  '    if false; then\n      python3 ' + self.script + '\n    fi')
        with self.assertRaisesRegex(AssertionError, 'unsupported coverage shell'):
            self.validate()

    def test_optional_workflow_step_is_not_accepted(self):
        self.edit('.github/ci/full-workflows/ci.yml', '      - name: Run locked test suite',
                  '      - name: Run locked test suite\n        if: false')
        with self.assertRaises(AssertionError):
            self.validate()

    def test_selector_id_on_another_step_is_not_an_output_route(self):
        self.edit('.github/ci/full-workflows/ci.yml', '        id: suites', '        id: not-suites')
        self.edit('.github/ci/full-workflows/ci.yml', '      - name: Run locked test suite',
                  '      - name: Run locked test suite\n        id: suites')
        with self.assertRaisesRegex(AssertionError, 'selector output'):
            self.validate()

    def test_missing_foundation_forwarding_is_rejected(self):
        self.edit('.github/scripts/check.sh',
                  'bash .github/scripts/check-foundation.sh "$1"',
                  'bash .github/scripts/check-foundation.sh foundation-platform')
        with self.assertRaisesRegex(AssertionError, 'forward'):
            self.validate()

    def test_browser_producer_dependency_is_required(self):
        self.edit('.github/ci/full-workflows/ci.yml',
                  '    needs: [static, web-production, smb-sdk]', '    needs: [static, smb-sdk]')
        with self.assertRaises(AssertionError):
            self.validate()

    def test_complex_existing_command_arguments_and_comments_are_preserved(self):
        commands = list(COVERAGE.shell_commands(
            'cargo test --locked --offline -p storage-engine --lib \\\n'
            '  disk::admission_tests::cold_directory_admission_keeps_held_identity_after_mount_replacement \\\n'
            '  -- --ignored --exact --test-threads=1\n'
            'python3 ".github/scripts/check-binding-expressions.py" --clang # real command\n'))
        self.assertEqual(commands[0][-4:], ['--', '--ignored', '--exact', '--test-threads=1'])
        self.assertEqual(commands[1], ['python3', '.github/scripts/check-binding-expressions.py', '--clang'])

    def test_unsupported_pipeline_is_not_silently_accepted(self):
        with self.assertRaisesRegex(AssertionError, 'unsupported coverage shell syntax'):
            list(COVERAGE.shell_commands('python3 ' + self.script + ' || true'))


class FoundationCoverageMapTests(unittest.TestCase):
    def test_generated_json_and_markdown_are_self_contained_recipe_declarations(self):
        expected = {
            'kind': 'retained-full-recipe',
            'workflow': '.github/ci/full-workflows/ci.yml',
            'execution_evidence': False,
            'current_policy': '.github/scripts/development-ci.py',
        }
        self.assertEqual(MAP['scope'], expected)
        self.assertEqual(COVERAGE.SCOPE, expected)
        self.assertEqual(json.loads(COVERAGE.json_text(MAP)), MAP)
        markdown = COVERAGE.markdown(MAP)
        self.assertEqual(markdown, (ROOT / 'docs/validation/foundation-test-coverage-map.md').read_text())
        for text in ('保存的完整 CI 配方', '不是当前开发 CI 的执行覆盖或通过证明',
                     '.github/ci/full-workflows/ci.yml', '人工全量 CI 与暂停边界'):
            self.assertIn(text, markdown)
        self.assertNotIn('| 已执行 |', markdown)
        self.assertNotIn('## 已执行', markdown)
        self.assertIn('完整配方含执行门禁', markdown)
        documentation = (ROOT / 'docs/validation/foundation-test-coverage.md').read_text()
        self.assertIn('retained_recipe_scripts()', documentation)
        self.assertIn('scope.execution_evidence', documentation)
        self.assertNotIn('可达性检查从 `ci.yml`', documentation)

    def test_generated_rendering_rejects_missing_or_execution_claiming_scope(self):
        for mode in ('missing', 'claim'):
            changed = copy.deepcopy(MAP)
            if mode == 'missing':
                del changed['scope']
            else:
                changed['scope']['execution_evidence'] = True
            for render in (COVERAGE.markdown, COVERAGE.json_text):
                with self.subTest(mode=mode, render=render.__name__), self.assertRaises((AssertionError, KeyError)):
                    render(changed)

    def test_tls_security_consumer_is_executed_without_claiming_upstream_suites(self):
        root = tomllib.loads((ROOT / "Cargo.toml").read_text())
        self.assertIn("src/client", root["workspace"]["default-members"])
        client = tomllib.loads((ROOT / "src/client/Cargo.toml").read_text())
        target = next(target for target in client["test"] if target["name"] == "tls_trust_regression")
        self.assertEqual(target["path"], "../../test/client/integration/tls_trust_regression.rs")
        self.assertTrue((ROOT / "src/client" / target["path"]).is_file())
        workspace = (ROOT / ".github/scripts/check.sh").read_text().split("  workspace)\n", 1)[1].split(";;", 1)[0]
        self.assertIn("    cargo test --locked --offline\n", workspace)
        packages = {package["package"]: package for package in MAP["packages"]}
        for name in ("rustls-webpki", "webpki-roots", "rustls-pki-types", "rustls", "hyper-rustls", "tokio-rustls"):
            with self.subTest(package=name):
                package = packages[name]
                self.assertEqual(package["classification"], "executed")
                gates = [gate for gate in package["gates"] if gate["shard"] == "workspace"]
                self.assertEqual(len(gates), 1)
                self.assertEqual(gates[0]["kind"], "P")
                self.assertEqual(gates[0]["scripts"], [".github/scripts/check.sh"])
                self.assertIn("test/client/integration/tls_trust_regression.rs", gates[0]["detail"])
                self.assertIn("upstream tests NOT run", gates[0]["detail"])

    def test_every_active_foundation_package_is_mapped_once(self):
        active = sorted(d for d in CATALOG["packages"] if d.startswith("src/foundation/"))
        self.assertEqual([package["directory"] for package in MAP["packages"]], active)
        self.assertEqual(len({package["package"] for package in MAP["packages"]}), len(active))

    def test_classification_follows_gates_and_profiles(self):
        for package in MAP["packages"]:
            kinds = {gate["kind"] for gate in package["gates"]}
            production = bool(set(package["profiles"]) & PRODUCTION)
            expected = ("executed" if kinds & {"U", "P", "F"} else
                        "needed-not-executed" if production else "reference-only")
            self.assertEqual(package["classification"], expected, package["package"])
            self.assertEqual(package["risk"] is None, expected != "needed-not-executed", package["package"])

    def test_behaviour_suites_preserve_retained_and_project_authored_classifications(self):
        packages = {package["package"]: package for package in MAP["packages"]}
        retained = set(BEHAVIOUR.UNIT_SUITES) | {suite.subject for suite in BEHAVIOUR.STANDALONE_SUITES}
        for name in retained:
            self.assertEqual(packages[name]["classification"], "executed", name)
            self.assertIn("U", {gate["kind"] for gate in packages[name]["gates"]}, name)
        for suite in BEHAVIOUR.INTEGRATION_SUITES:
            package = packages[suite.subject]
            self.assertEqual(package["classification"], "executed", suite.subject)
            expected = {"U" if path.startswith("test/foundation/") else "P" for path in suite.tests}
            if suite.subject in retained:
                expected.add("U")
            recorded = {gate["kind"] for gate in package["gates"]
                        if gate["scripts"] == [".github/scripts/check-foundation-behaviour.py"]}
            self.assertEqual(recorded, expected, suite.subject)

    def test_retired_records_are_listed_without_live_packages(self):
        records = {record["record"] for record in MAP["retired"]}
        self.assertEqual(records, set(CATALOG["retired"]) | set(CATALOG["superseded"]))
        self.assertFalse(records & set(CATALOG["packages"]))


if __name__ == "__main__":
    unittest.main()
