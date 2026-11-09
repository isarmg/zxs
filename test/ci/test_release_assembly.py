"""Reject release-set redirection and extra inputs before generating metadata."""
from pathlib import Path
import os
import shutil
import subprocess
import tempfile
import unittest
from web_dist_fixture import write_release_assets, bundle_member

ROOT = Path(__file__).resolve().parents[2]


class ReleaseAssembly(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        scripts = self.root / '.github/scripts'
        scripts.mkdir(parents=True)
        shutil.copy2(ROOT / '.github/scripts/release-assemble.sh', scripts)
        for name in ('verify-release-assets.py', 'release-source.py', 'release-source-bundle.py'):
            shutil.copy2(ROOT / '.github/scripts' / name, scripts)
        self.assets = self.root / 'release-assets'
        self.assets.mkdir()
        self.commit = write_release_assets(self.assets)['commit']
        self.source = self.assets / 'zxs-source-0.1.0.tar.gz'
        self.build = self.source.read_bytes()
        binary = self.root / 'bin'
        binary.mkdir()
        git = binary / 'git'
        git.write_text('#!/bin/sh\nprintf "' + self.commit + '\\n"\n')
        git.chmod(0o755)
        self.environment = os.environ | {'PATH': str(binary) + os.pathsep + os.environ['PATH']}

    def reject(self, version='0.1.0', error=None):
        result = subprocess.run(['bash', str(self.root / '.github/scripts/release-assemble.sh')],
                                env=self.environment | {'RELEASE_VERSION': version}, capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        if error is not None:
            self.assertIn(error, result.stderr.decode())
        self.assertFalse((self.assets / 'INSTALL.md').exists())
        if self.source.exists():
            self.assertEqual(self.source.read_bytes(), self.build)
        self.assertFalse((self.assets / 'SHA256SUMS').exists())

    def test_unexpected_hidden_entry_is_not_ignored(self):
        (self.assets / '.unreviewed').write_text('not a release asset')
        self.reject(error='exactly ZXS, helper and complete source')

    def test_unexpected_directory_is_not_ignored(self):
        (self.assets / 'extra').mkdir()
        self.reject(error='exactly ZXS, helper and complete source')

    def test_archive_symlink_cannot_read_outside_release(self):
        archive = self.source
        archive.rename(self.root / 'outside')
        archive.symlink_to(self.root / 'outside')
        self.reject()

    def test_asset_directory_symlink_cannot_redirect_outputs(self):
        self.assets.rename(self.root / 'outside')
        self.assets.symlink_to(self.root / 'outside', target_is_directory=True)
        self.reject()

    def test_invalid_version_is_rejected_before_metadata(self):
        self.reject('../outside', error='invalid release version')

    def test_missing_native_receipt_is_rejected_instead_of_reconstructed(self):
        bundle_member(self.assets, 'BUILD.txt').unlink()
        self.build = self.source.read_bytes()
        self.reject(error='source bundle lacks')

    def test_wrong_target_is_rejected_without_overwriting_build_receipt(self):
        build = bundle_member(self.assets, 'BUILD.txt')
        build.write_bytes(build.read_bytes().replace(b'x86_64-unknown-linux-gnu', b'x86_64-unknown-linux-musl'))
        self.build = self.source.read_bytes()
        self.reject(error='native build target/profile identity mismatch')


if __name__ == '__main__':
    unittest.main()

