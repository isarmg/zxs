"""Crypto gate fixtures, feature wiring, closure and authenticated origin archive."""
from pathlib import Path
import base64
import gzip
import hashlib
import io
import json
import runpy
import tempfile
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]
GATE = runpy.run_path(str(ROOT / '.github/scripts/check-crypto-consolidation.py'))
ARCHIVE_SHA256 = '98c1a4d23992c31dc9e84ea0b09aba4714fd9d1c1a7db4d123f06eb93caa0be5'


class CryptoConsolidationTests(unittest.TestCase):
    def test_manifest_features_are_real_and_no_registry_sources(self):
        manifest = tomllib.loads(GATE['manifest'](ROOT))
        for name, dependency in manifest['dependencies'].items():
            self.assertFalse(dependency['default-features'])
            self.assertEqual(set(dependency), {'path', 'default-features'})
            actual = tomllib.loads((Path(dependency['path']) / 'Cargo.toml').read_text())
            self.assertEqual(actual['package']['name'], name)
        for values in manifest['features'].values():
            for value in values:
                if '/' in value:
                    name, feature = value.split('/')
                    actual = tomllib.loads((Path(manifest['dependencies'][name]['path']) / 'Cargo.toml').read_text())
                    features = set(actual.get('features', {}))
                    activated = {v for values in actual.get('features', {}).values() for v in values}
                    features.update(name for name, dep in actual.get('dependencies', {}).items() if dep.get('optional') and 'dep:' + name not in activated)
                    self.assertIn(feature, features)

    def test_blake3_feature_contract_is_preserved(self):
        component = tomllib.loads((ROOT / 'src/foundation/cryptography/blake3/Cargo.toml').read_text())
        self.assertNotIn('digest', component['features'])
        self.assertEqual(component['features']['cryptographic-core'], ['dep:cryptographic-core'])
        self.assertEqual(component['features']['traits-preview'], ['dep:cryptographic-core'])
        manifest = tomllib.loads(GATE['manifest'](ROOT))
        self.assertEqual(manifest['features']['cryptographic-core'], ['blake3/cryptographic-core'])
        self.assertEqual(manifest['features']['traits-preview'], ['blake3/traits-preview'])

    def test_duplicate_stack_is_rejected(self):
        metadata = {'packages': [{'name': name} for name in ('cryptographic-core', 'hybrid-array')]}
        GATE['check_shared_stack'](ROOT, metadata)
        for name in ('cryptographic-core', 'hash-traits', 'hash-block-buffer', 'cryptographic-traits', 'digest', 'block-buffer', 'crypto-common', 'generic-array'):
            with self.subTest(name=name):
                invalid = {'packages': metadata['packages'] + [{'name': name}]}
                with self.assertRaises(AssertionError):
                    GATE['check_shared_stack'](ROOT, invalid)

    def test_bounded_independent_generator_is_reproducible(self):
        generate = runpy.run_path(str(ROOT / 'test/ci/crypto_vectors.py'))['generate']
        with tempfile.TemporaryDirectory() as temporary:
            work = Path(temporary)
            counts = generate(ROOT, work)
            before = {p.name: p.read_bytes() for p in (work / 'tests').iterdir()}
            self.assertEqual(counts, {'upstream_vectors': 518, 'independent_vectors': 1296})
            self.assertLess(len(before['vectors.rs']), 1024 * 1024)
            self.assertEqual(generate(ROOT, work), counts)
            self.assertEqual(before, {p.name: p.read_bytes() for p in (work / 'tests').iterdir()})

    def test_retired_and_merged_originals_remain_authenticated(self):
        compressed = (ROOT / 'compliance/crypto-consolidation-source-archive.json.gz').read_bytes()
        self.assertLess(len(compressed), 1024 * 1024)
        self.assertEqual(hashlib.sha256(compressed).hexdigest(), ARCHIVE_SHA256)
        with gzip.GzipFile(fileobj=io.BytesIO(compressed)) as stream:
            raw = stream.read(4 * 1024 * 1024 + 1)
            self.assertLessEqual(len(raw), 4 * 1024 * 1024)
            self.assertEqual(stream.read(1), b'')
        archive = json.loads(raw)
        self.assertEqual(archive['schema_version'], 1)
        names = ('digest-0.10.7', 'block-buffer-0.10.4', 'crypto-common-0.1.7', 'generic-array')
        self.assertEqual(set(archive['owners']), {'src/foundation/' + name for name in names})
        catalog = json.loads((ROOT / 'compliance/rust-sources.json').read_text())
        for directory, owner in archive['owners'].items():
            package = owner['package']
            expected = {directory + '/' + p: h for p, h in package['files_sha256'].items()}
            for row in (owner['external_tests'] or {}).get('files', []):
                expected[row['path']] = row['sha256']
            self.assertEqual(set(expected), set(owner['files']))
            for relative, data in owner['files'].items():
                self.assertEqual(hashlib.sha256(base64.b64decode(data, validate=True)).hexdigest(), expected[relative])
                self.assertFalse((ROOT / relative).exists(), relative)
            if directory.endswith('/digest-0.10.7'):
                matches = [m for m in catalog['merges'] if m['origin_directory'] == package['origin_directory']]
                self.assertEqual(len(matches), 1)
                self.assertEqual(matches[0]['directory'], 'src/foundation/cryptography/blake2')
                self.assertEqual(matches[0]['upstream'], package['upstream'])
                self.assertEqual(matches[0]['merge_receipt']['directory'], directory)
                self.assertEqual(matches[0]['merge_receipt']['files_sha256'], package['files_sha256'])
                self.assertEqual(matches[0]['license_materials'], package['license_materials'])
                self.assertNotIn(directory, catalog['retired'])
            else:
                retired = {k: v for k, v in catalog['retired'][directory].items() if k != 'retirement'}
                self.assertEqual(retired, package)


if __name__ == '__main__':
    unittest.main()
