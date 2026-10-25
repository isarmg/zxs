"""Static architectural boundary fixtures: no compiler, metadata or registry."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / '.github/scripts/check-architecture.py'
SPEC = importlib.util.spec_from_file_location('architecture_policy', SCRIPT)
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)


class ArchitecturePolicyTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='architecture-policy-')
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.policy = {'version': 1, 'owners': {}}
        self.members = ['src/a', 'src/b']
        for name in ('a', 'b'):
            self.package('src/' + name, name)
            self.policy['owners'][name] = {'path': 'src/' + name, 'runtime': [], 'build': [], 'dev': []}
        self.workspace()

    def package(self, path, name, extra=''):
        directory = self.root / path
        directory.mkdir(parents=True, exist_ok=True)
        (directory / 'Cargo.toml').write_text(f'[package]\nname = "{name}"\nversion = "0.1.0"\n' + extra)

    def workspace(self, extra=''):
        (self.root / 'Cargo.toml').write_text('[workspace]\nmembers = ' + json.dumps(self.members) + '\n' + extra)

    def check(self):
        path = self.root / 'policy.json'
        path.write_text(json.dumps(self.policy))
        return CHECK.validate(self.root, path)

    def reject(self, message):
        with self.assertRaisesRegex(CHECK.PolicyError, message):
            self.check()

    def test_repository_inventory(self):
        self.assertEqual(CHECK.validate(ROOT), (20, 29))

    def test_only_engine_dev_self_edge_is_approved(self):
        for scope in ('runtime', 'build', 'dev'):
            self.policy['owners']['a'][scope] = ['a']
            self.reject('self dependency is forbidden')
            self.policy['owners']['a'][scope] = []
        self.members = ['src/engine']
        self.package('src/engine', 'storage-engine',
                     '[dev-dependencies]\nstorage-engine = { path = ".", features = ["fault-injection"] }\n')
        self.policy['owners'] = {'storage-engine': {
            'path': 'src/engine', 'runtime': [], 'build': [], 'dev': ['storage-engine']}}
        self.workspace()
        self.assertEqual(self.check(), (1, 1))
        for scope in ('runtime', 'build'):
            self.policy['owners']['storage-engine'][scope] = ['storage-engine']
            self.reject('self dependency is forbidden')
            self.policy['owners']['storage-engine'][scope] = []

    def test_leaf_packages(self):
        self.assertEqual(self.check(), (2, 0))

    def test_optional_target_alias_resolves_actual_package(self):
        self.package('src/a', 'a', '\n[target.\'cfg(windows)\'.dependencies]\nrenamed = { package = "b", path = "../b", optional = true }\n')
        self.reject('forbidden runtime edge a -> b')
        self.policy['owners']['a']['runtime'] = ['b']
        self.assertEqual(self.check(), (2, 1))

    def test_all_target_dependency_scopes(self):
        for table, scope in [('dependencies', 'runtime'), ('build-dependencies', 'build'), ('dev-dependencies', 'dev')]:
            with self.subTest(scope=scope):
                self.policy['owners']['a'].update(runtime=[], build=[], dev=[])
                self.package('src/a', 'a', f'\n[target.\'cfg(unix)\'.{table}]\nb = {{ path = "../b" }}\n')
                self.reject(f'forbidden {scope} edge')
                self.policy['owners']['a'][scope] = ['b']
                self.check()

    def test_workspace_alias_path_is_relative_to_workspace(self):
        self.workspace('[workspace.dependencies]\nalias = { package = "b", path = "src/b" }\n')
        self.package('src/a', 'a', '[dependencies]\nalias = { workspace = true, optional = true }\n')
        self.reject('forbidden runtime edge')
        self.policy['owners']['a']['runtime'] = ['b']
        self.assertEqual(self.check(), (2, 1))

    def test_inherited_source_cannot_be_overridden(self):
        self.workspace('[workspace.dependencies]\nb = { path = "src/b" }\n')
        self.package('src/a', 'a', '[dependencies]\nb = { workspace = true, path = "../b" }\n')
        self.reject('source cannot be overridden')

    def test_missing_inherited_dependency(self):
        self.package('src/a', 'a', '[dependencies]\nb.workspace = true\n')
        self.reject('invalid workspace inheritance')

    def test_dev_approval_does_not_approve_runtime_or_build(self):
        self.policy['owners']['a']['dev'] = ['b']
        for table in ('dependencies', 'build-dependencies'):
            self.package('src/a', 'a', f'[{table}]\nb = {{ path = "../b" }}\n')
            self.reject('forbidden')

    def test_dev_backedge_is_allowed_but_build_cycle_is_not(self):
        self.policy['owners']['a']['runtime'] = ['b']
        self.policy['owners']['b']['dev'] = ['a']
        self.package('src/a', 'a', '[dependencies]\nb = { path = "../b" }\n')
        self.package('src/b', 'b', '[dev-dependencies]\na = { path = "../a" }\n')
        self.check()
        self.policy['owners']['b']['build'] = ['a']
        self.package('src/b', 'b', '[target.\'cfg(windows)\'.build-dependencies]\na = { path = "../a", optional = true }\n')
        self.reject('runtime/build dependency cycle: a -> b -> a')

    def test_new_member_is_not_automatically_approved(self):
        self.package('src/new', 'new')
        self.members.append('src/new')
        self.workspace()
        self.reject('undeclared first-party member')

    def test_implicit_root_package_is_rejected(self):
        self.workspace('[package]\nname = "implicit-root"\nversion = "0.1.0"\n')
        self.reject('must remain a virtual workspace')

    def test_implicit_path_member_is_not_automatically_approved(self):
        # Cargo can auto-add this in-tree path dependency to its workspace even
        # though it is not spelled out in workspace.members.
        self.package('src/implicit', 'implicit')
        self.package('src/a', 'a', '[dependencies]\nimplicit = { path = "../implicit" }\n')
        self.reject('undeclared first-party source')

    def test_stale_owner(self):
        self.members.remove('src/b')
        self.workspace()
        self.reject('stale rule owners.*b')

    def test_unknown_rule_target(self):
        self.policy['owners']['a']['runtime'] = ['missing']
        self.reject('unknown rule owner')

    def test_duplicate_edges_and_malformed_scopes(self):
        for value, message in [(['b', 'b'], 'duplicate edge'), ('b', 'expected list'), ([4], 'expected list')]:
            self.policy['owners']['a']['runtime'] = value
            self.reject(message)
        self.policy['owners']['a']['runtime'] = []
        self.policy['owners']['a']['target'] = []
        self.reject('rule requires')

    def test_duplicate_policy_keys(self):
        path = self.root / 'duplicate.json'
        path.write_text('{"version": 1, "version": 1, "owners": {}}')
        with self.assertRaisesRegex(CHECK.PolicyError, 'duplicate policy key'):
            CHECK.validate(self.root, path)

    def test_policy_path_and_name_must_match(self):
        self.policy['owners']['a']['path'] = 'src/b'
        self.reject('path contains package b')

    def test_dependency_package_name_must_match_even_for_alias(self):
        self.package('src/a', 'a', '[dependencies]\nalias = { package = "not-b", path = "../b" }\n')
        self.reject('declared package not-b, path contains b')

    def test_external_source_cannot_impersonate_first_party(self):
        for spec in ['"1"', '{ package = "b", git = "https://invalid.example/b" }']:
            self.package('src/a', 'a', f'[dependencies]\nb = {spec}\n')
            self.reject('requires its approved local path')

    def test_unlisted_local_source_is_rejected(self):
        self.package('tools/unlisted', 'unlisted')
        self.package('src/a', 'a', '[dependencies]\nunlisted = { path = "../../tools/unlisted" }\n')
        self.reject('undeclared first-party source')

    def test_missing_absolute_and_escaping_paths(self):
        for path, message in [('../missing', 'missing Cargo.toml'), ('../../../outside', 'escapes repository'),
                              (str(self.root / 'src/b'), 'absolute path')]:
            self.package('src/a', 'a', f'[dependencies]\nb = {{ path = {json.dumps(path)} }}\n')
            self.reject(message)

    def test_symlink_directory_and_manifest_cannot_escape(self):
        with tempfile.TemporaryDirectory() as outside:
            external = Path(outside)
            (external / 'Cargo.toml').write_text('[package]\nname = "b"\n')
            (self.root / 'src/link').symlink_to(external, target_is_directory=True)
            self.package('src/a', 'a', '[dependencies]\nb = { path = "../link" }\n')
            self.reject('escapes repository')
            self.package('src/a', 'a', '[dependencies]\nb = { path = "../b" }\n')
            (self.root / 'src/b/Cargo.toml').unlink()
            (self.root / 'src/b/Cargo.toml').symlink_to(external / 'Cargo.toml')
            self.reject('manifest escapes repository')

    def test_foundation_scope_is_exact_and_internals_are_not_walked(self):
        self.package('src/foundation/vendor', 'vendor', '[dependencies]\nunresolvable = { path = "/not-inspected" }\n')
        self.members.append('src/foundation/vendor')
        self.workspace()
        self.package('src/a', 'a', '[dependencies]\nvendor = { path = "../foundation/vendor" }\n')
        self.check()
        self.package('src/foundation-lookalike/vendor', 'vendor')
        self.package('src/a', 'a', '[dependencies]\nvendor = { path = "../foundation-lookalike/vendor" }\n')
        self.reject('undeclared first-party source')

    def test_foundation_source_cannot_masquerade_as_first_party(self):
        self.package('src/foundation/copy', 'b')
        self.package('src/a', 'a', '[dependencies]\nb = { path = "../foundation/copy" }\n')
        self.policy['owners']['a']['runtime'] = ['b']
        self.reject('first-party package cannot use foundation source')

    def test_normalization_does_not_hide_forbidden_edge(self):
        (self.root / 'src/foundation').mkdir()
        self.package('src/a', 'a', '[dependencies]\nb = { path = "../foundation/../b" }\n')
        self.reject('forbidden runtime edge')

    def test_conflicting_sources_are_invalid(self):
        self.package('src/a', 'a', '[dependencies]\nb = { path = "../b", git = "https://invalid.example/b" }\n')
        self.reject('both path and git')

    def test_workspace_member_escape_and_glob_are_rejected(self):
        self.members.append('../outside')
        self.workspace()
        self.reject('escapes repository')
        self.members[-1] = 'src/*'
        self.workspace()
        self.reject('explicit paths')

    def test_cli_fails_cleanly_on_malformed_toml(self):
        (self.root / 'Cargo.toml').write_text('[invalid')
        path = self.root / 'policy.json'
        path.write_text(json.dumps(self.policy))
        result = subprocess.run([sys.executable, str(SCRIPT), '--root', str(self.root), '--policy', str(path)],
                                capture_output=True, text=True, check=False)
        self.assertEqual(result.returncode, 1)
        self.assertIn('architecture:', result.stderr)
        self.assertNotIn('Traceback', result.stderr)


if __name__ == '__main__':
    unittest.main()
