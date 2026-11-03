"""The isolated serialization gate uses valid current features and fail-closed probes."""
from pathlib import Path
from copy import deepcopy
import runpy
import subprocess
import tempfile
import tomllib
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
CHECKER = runpy.run_path(str(ROOT / '.github/scripts/check-serialization-family.py'))


class SerializationGateTests(unittest.TestCase):
    def test_source_paths_include_derive_bound_strings(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'src').mkdir()
            source = root / 'src/describe.rs'
            source.write_text('use erased_serde::Serializer; // _serde::local_alias\n')
            CHECKER['check_source_paths'](root)
            source.write_text('#[serialization(bound(deserialize = "T: serde::de::DeserializeOwned"))]')
            with self.assertRaisesRegex(AssertionError, 'old serialization crate path'):
                CHECKER['check_source_paths'](root)
        CHECKER['check_source_paths'](ROOT)

    def test_private_family_requires_matching_versions_and_exact_pins(self):
        names = ('data-serialization', 'serialization-derive')
        manifests = {name: {'package': {'version': '1.2.3'}} for name in names}
        manifests[names[0]]['dependencies'] = {name: {'version': '=1.2.3'} for name in names[1:]}
        CHECKER['validate_private_family'](manifests)
        for name in names[1:]:
            with self.subTest(name=name, case='version'):
                invalid = deepcopy(manifests)
                invalid[name]['package']['version'] = '1.2.4'
                with self.assertRaisesRegex(AssertionError, 'versions must match'):
                    CHECKER['validate_private_family'](invalid)
            with self.subTest(name=name, case='pin'):
                invalid = deepcopy(manifests)
                invalid[names[0]]['dependencies'][name]['version'] = '1'
                with self.assertRaisesRegex(AssertionError, 'must be pinned'):
                    CHECKER['validate_private_family'](invalid)

    def test_manifest_features_match_internal_packages(self):
        for no_std in (False, True):
            dependencies = tomllib.loads(CHECKER['manifest'](ROOT, no_std))['dependencies']
            self.assertEqual('semver' in dependencies, not no_std)
            for name, dependency in dependencies.items():
                package = tomllib.loads((Path(dependency['path']) / 'Cargo.toml').read_text())
                self.assertEqual(package['package']['name'], name)
                self.assertTrue(set(dependency['features']) <= set(package.get('features', {})))
            if no_std:
                self.assertTrue(all(not dependency['default-features'] for dependency in dependencies.values()))

    def test_build_honors_explicit_isolated_target_directory(self):
        with patch.object(CHECKER['subprocess'], 'Popen') as popen:
            process = popen.return_value.__enter__.return_value
            process.stdout = ['{"reason":"build-finished","success":true}\n']
            process.wait.return_value = 0
            CHECKER['build'](ROOT, Path('/tmp/serialization-probe'),
                             {'CARGO_TARGET_DIR': '/tmp/consumer-target'}, CHECKER['NATIVE'])
            command = popen.call_args.args[0]
            self.assertEqual(command[command.index('--target-dir') + 1], '/tmp/consumer-target')
            self.assertIn('-j2', command)

    def test_rejection_target_exists_before_lock_generation(self):
        def run(command, **kwargs):
            manifest = Path(command[command.index('--manifest-path') + 1])
            source = manifest.parent / 'src/lib.rs'
            self.assertTrue(source.is_file())
            if command[1] == 'generate-lockfile':
                return subprocess.CompletedProcess(command, 0)
            error = ('cannot find attribute `serde`' if '#[serde(' in source.read_text()
                     else 'serialization_core')
            return subprocess.CompletedProcess(command, 101, '', error)

        with tempfile.TemporaryDirectory() as directory:
            with patch.object(CHECKER['subprocess'], 'run', side_effect=run) as mocked:
                CHECKER['check_rejections'](ROOT, Path(directory), {})
            self.assertEqual(mocked.call_count, 3)

    def test_unexpected_success_does_not_pass_rejection_gate(self):
        with tempfile.TemporaryDirectory() as directory:
            with patch.object(CHECKER['subprocess'], 'run', return_value=subprocess.CompletedProcess([], 0, '', '')):
                with self.assertRaises(AssertionError):
                    CHECKER['check_rejections'](ROOT, Path(directory), {})


if __name__ == '__main__':
    unittest.main()
