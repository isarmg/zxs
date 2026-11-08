"""Checks for the consolidated core's bounded probes and immutable origins."""
from pathlib import Path
import base64
import gzip
import hashlib
import io
import json
import runpy
import re
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]
foundation_path = runpy.run_path(str(ROOT / '.github/scripts/foundation_paths.py'))['package_path']
CHECKS = runpy.run_path(str(ROOT / 'test/ci/crypto_core_checks.py'))
ARCHIVE_SHA256 = '27cad88e27ab5f5dbfaa6333f980486667667bac89f2f3109fe41b46c3071c19'


class CryptographicCoreChecks(unittest.TestCase):
    def test_minimal_consumer_uses_only_supported_features(self):
        manifest = tomllib.loads(CHECKS['manifest'](ROOT / 'src/foundation/cryptography/cryptographic-core'))
        self.assertEqual(set(manifest['dependencies']), {'cryptographic-core'})
        self.assertFalse(manifest['dependencies']['cryptographic-core']['default-features'])
        core = tomllib.loads((ROOT / 'src/foundation/cryptography/cryptographic-core/Cargo.toml').read_text())
        self.assertEqual(core['package']['name'], 'cryptographic-core')
        self.assertEqual(core['features']['block-api'], [])
        self.assertEqual(set(manifest['features']), set(core['features']))
        self.assertNotIn('std', core['features'])
        for name in manifest['features']:
            if name != 'default':
                self.assertIn(name, core['features'])
        self.assertEqual(len(CHECKS['FEATURES']), 8)
        self.assertNotIn('rand_core', core['features'])
        self.assertNotIn('getrandom', core['features'])

    def test_buffer_fixture_is_bounded_and_preserves_cases(self):
        generated = CHECKS['buffer_fixture'](ROOT)
        original = (ROOT / 'test/foundation/cryptographic-core/buffer/tests/mod.rs').read_text()
        self.assertEqual(original.count('#[test]'), 11)
        self.assertEqual(generated.count('#[test]'), 11)
        self.assertNotIn('hex!', generated)
        self.assertNotIn('hex_literal', generated)
        self.assertEqual(generated, CHECKS['buffer_fixture'](ROOT))
        self.assertLess(len(generated.encode()), 32768)

    def test_no_retained_imports_use_absorbed_package_names(self):
        obsolete = re.compile(r"\b(?:hash_traits|hash_block_buffer|cryptographic_traits|crypto_common|block_buffer)::")
        for parent in ('src', 'test', 'helper'):
            for path in (ROOT / parent).rglob('*.rs'):
                self.assertIsNone(obsolete.search(path.read_text()), str(path))
                self.assertIsNone(re.search(r'use\s+(?:crate|cryptographic_core)::\{\s*crate::', path.read_text()), str(path))
        for owner in ('cryptographic-core', 'blake2', 'sha2', 'hmac', 'blake3', 'argon2'):
            for parent in (foundation_path(ROOT, owner), ROOT / 'test/foundation' / owner):
                for path in parent.rglob('*.rs'):
                    self.assertIsNone(re.search(r"\bdigest::", path.read_text()), str(path))

    def test_three_original_implementations_remain_authenticated(self):
        data = (ROOT / 'compliance/cryptographic-core-source-archive.json.gz').read_bytes()
        self.assertLess(len(data), 1024 * 1024)
        self.assertEqual(hashlib.sha256(data).hexdigest(), ARCHIVE_SHA256)
        with gzip.GzipFile(fileobj=io.BytesIO(data)) as stream:
            payload = stream.read(4 * 1024 * 1024 + 1)
        self.assertLessEqual(len(payload), 4 * 1024 * 1024)
        archive = json.loads(payload)
        names = ('digest-0.11.3', 'block-buffer-0.12.1', 'crypto-common-0.2.2')
        self.assertEqual(set(archive['owners']), {'src/foundation/' + name for name in names})
        catalog = json.loads((ROOT / 'compliance/rust-sources.json').read_text())
        current = catalog['packages']['src/foundation/cryptography/cryptographic-core']
        retained = [current] + [row for row in catalog['merges'] if row['directory'] == current['directory']]
        self.assertEqual(len(retained), 3)
        for directory, original in archive['owners'].items():
            package = original['package']
            self.assertEqual(directory, package['directory'])
            owners = [row for row in retained if row['origin_directory'] == package['origin_directory']]
            self.assertEqual(len(owners), 1)
            for field in ('upstream', 'license', 'license_materials', 'vcs', 'version'):
                self.assertEqual(owners[0][field], package[field], field)
            self.assertNotIn(directory, catalog['retired'])
            expected = {directory + '/' + name: value for name, value in package['files_sha256'].items()}
            external = original['external_tests']
            if external:
                expected.update({row['path']: row['sha256'] for row in external['files']})
            self.assertEqual(set(original['files']), set(expected))
            for path, encoded in original['files'].items():
                content = base64.b64decode(encoded, validate=True)
                self.assertEqual(hashlib.sha256(content).hexdigest(), expected[path])
            self.assertFalse((ROOT / directory).exists())
            for license_path in package['license_materials']:
                # The original inventory, rather than a guessed consolidated license, owns attribution.
                self.assertTrue((ROOT / license_path).is_file(), license_path)
                self.assertGreater((ROOT / license_path).stat().st_size, 100)


if __name__ == '__main__':
    unittest.main()
