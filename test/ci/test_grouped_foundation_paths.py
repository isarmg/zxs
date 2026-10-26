"""Grouped sources remain discoverable and isolated CI mirrors keep real inputs."""
import importlib.util
import json
from pathlib import Path
import runpy
import tempfile
import tomllib
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
PATHS = runpy.run_path(str(ROOT / '.github/scripts/foundation_paths.py'))


def load(name):
    spec = importlib.util.spec_from_file_location(name.replace('-', '_'),
                                                ROOT / '.github/scripts' / (name + '.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class ManifestCaptured(Exception):
    """Stop before any compiler, resolver, or external process is invoked."""


class GroupedFoundationPaths(unittest.TestCase):
    def test_every_current_package_is_discovered_at_its_grouped_root(self):
        catalog = json.loads((ROOT / 'compliance/rust-sources.json').read_text())
        packages = {name: row for name, row in catalog['packages'].items()
                    if name.startswith('src/foundation/')}
        self.assertEqual(len(packages), 222)
        for directory, row in packages.items():
            with self.subTest(package=directory):
                path = Path(directory)
                self.assertEqual(len(path.parts), 4)
                self.assertEqual(PATHS['package_path'](ROOT, path.name), ROOT / path)
                manifest = tomllib.loads((ROOT / path / 'Cargo.toml').read_text())
                self.assertEqual(manifest['package']['version'], row['version'])
        self.assertFalse(list((ROOT / 'src/foundation').glob('*/Cargo.toml')))

    def test_nested_sources_and_retired_existence_checks_use_grouped_paths(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            package = root / 'src/foundation/example-group/example'
            package.mkdir(parents=True)
            self.assertEqual(PATHS['package_path'](root, 'example/src/lib.rs'),
                             package / 'src/lib.rs')
            retired = root / 'src/foundation/example-group/retired'
            retired.mkdir()
            self.assertTrue(PATHS['package_path'](root, 'retired').exists())
            self.assertFalse(PATHS['package_path'](root, 'absent').exists())

    def test_duplicate_roots_and_path_traversal_are_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for group in ('one', 'two'):
                (root / 'src/foundation' / group / 'duplicate').mkdir(parents=True)
            with self.assertRaisesRegex(ValueError, 'ambiguous'):
                PATHS['package_path'](root, 'duplicate')
            for path in ('../outside', '/outside', '*', 'name[0]', '?'):
                with self.assertRaisesRegex(ValueError, 'invalid'):
                    PATHS['package_path'](root, path)

    def test_rebasing_resolves_same_and_cross_group_dependencies_only(self):
        source = Path('/repository/src/foundation/concurrency/futures')
        manifest = '''[dependencies]
local = { path = "../futures-macro", optional = true }
[target.'cfg(unix)'.dependencies]
other = { path = "../../collections/slab" }
[build-dependencies.helper]
path = '../../tooling/autocfg'
[dev-dependencies.fixture]
path = "../../../../test/fixture"
[lib]
path = "src/lib.rs"
[[test]]
name = "retained"
path = "../../../../test/foundation/futures/tests/join.rs"
'''
        parsed = tomllib.loads(PATHS['rebase_dependency_paths'](manifest, source))
        self.assertEqual(parsed['dependencies']['local']['path'],
                         '/repository/src/foundation/concurrency/futures-macro')
        self.assertEqual(parsed['target']['cfg(unix)']['dependencies']['other']['path'],
                         '/repository/src/foundation/collections/slab')
        self.assertEqual(parsed['build-dependencies']['helper']['path'],
                         '/repository/src/foundation/tooling/autocfg')
        self.assertEqual(parsed['dev-dependencies']['fixture']['path'], '/repository/test/fixture')
        self.assertEqual(parsed['lib']['path'], 'src/lib.rs')
        self.assertEqual(parsed['test'][0]['path'],
                         '../../../../test/foundation/futures/tests/join.rs')

    def test_rebasing_does_not_touch_identical_test_paths_or_comments(self):
        manifest = '''[dependencies.local]
path = "../same"
# path = "../same"
[[test]]
name = "same-spelling"
path = "../same"
[package.metadata.dependencies]
path = "../same"
'''
        rebased = PATHS['rebase_dependency_paths'](manifest, '/repo/category/package')
        data = tomllib.loads(rebased)
        self.assertEqual(data['dependencies']['local']['path'], '/repo/category/same')
        self.assertEqual(data['test'][0]['path'], '../same')
        self.assertEqual(data['package']['metadata']['dependencies']['path'], '../same')
        self.assertIn('# path = "../same"', rebased)

    def assert_dependencies_exist(self, manifest):
        def visit(value):
            if not isinstance(value, dict):
                return
            for key, child in value.items():
                if key in ('dependencies', 'dev-dependencies', 'build-dependencies'):
                    for row in child.values():
                        if isinstance(row, dict) and 'path' in row:
                            path = Path(row['path'])
                            self.assertTrue(path.is_absolute(), row)
                            self.assertTrue((path / 'Cargo.toml').is_file(), row)
                else:
                    visit(child)
        visit(tomllib.loads(manifest.read_text()))

    def assert_targets_exist(self, manifest):
        data = tomllib.loads(manifest.read_text())
        for kind in ('lib', 'bin', 'test', 'bench', 'example'):
            targets = data.get(kind, [])
            if isinstance(targets, dict):
                targets = [targets]
            for target in targets:
                if 'path' in target:
                    self.assertTrue((manifest.parent / target['path']).is_file(), target)

    def capture_manifest(self, command, **kwargs):
        manifest = Path(command[command.index('--manifest-path') + 1])
        self.assert_dependencies_exist(manifest)
        self.assert_targets_exist(manifest)
        raise ManifestCaptured

    def test_foundation_consumer_uses_actual_grouped_providers(self):
        gate = load('check-foundation-consumers')
        with mock.patch.object(gate, 'check_prop_parser'), \
                mock.patch.object(gate.subprocess, 'run', side_effect=self.capture_manifest):
            with self.assertRaises(ManifestCaptured):
                gate.check(ROOT)

    def test_retained_futures_and_http_body_mirrors_preserve_depth(self):
        for name in ('check-futures-consolidation', 'check-http-body-consolidation'):
            gate = load(name)
            with self.subTest(gate=name), tempfile.TemporaryDirectory() as temporary:
                def capture(root, command, **kwargs):
                    self.capture_manifest(command, **kwargs)
                with mock.patch.object(gate, 'run', side_effect=capture):
                    with self.assertRaises(ManifestCaptured):
                        gate.check_retained_tests(ROOT, Path(temporary), Path(temporary) / 'target')

    def test_pattern_and_sqlite_mirrors_keep_all_live_inputs(self):
        for name, prepare in (('check-pattern-matching', 'fixture'),
                              ('check-sqlite-family', 'prepare_package')):
            gate = load(name)
            with self.subTest(gate=name), tempfile.TemporaryDirectory() as temporary:
                manifest = getattr(gate, prepare)(ROOT, Path(temporary))
                self.assert_dependencies_exist(manifest)
                self.assert_targets_exist(manifest)

    def test_compression_and_wasm_mirrors_retain_external_tests(self):
        for name in ('check-compression-family', 'check-wasm-binding-macros'):
            gate = load(name)
            with self.subTest(gate=name), \
                    mock.patch.object(gate.subprocess, 'run', side_effect=self.capture_manifest):
                with self.assertRaises(ManifestCaptured):
                    gate.check()


if __name__ == '__main__':
    unittest.main()
