"""Reject release-set redirection and extra inputs before generating metadata."""
from pathlib import Path
import os
import shutil
import subprocess
import tempfile
import unittest
from web_dist_fixture import write_release_assets

ROOT = Path(__file__).resolve().parents[2]


class ReleaseAssembly(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        scripts = self.root / '.github/scripts'
        scripts.mkdir(parents=True)
        shutil.copy2(ROOT / '.github/scripts/release-assemble.sh', scripts)
        shutil.copy2(ROOT / '.github/scripts/verify-release-assets.py', scripts)
        self.assets = self.root / 'release-assets'
        self.assets.mkdir()
        write_release_assets(self.assets, 'a' * 40)
        (self.assets / 'INSTALL.md').unlink()
        self.build = (self.assets / 'BUILD.txt').read_bytes()
        binary = self.root / 'bin'
        binary.mkdir()
        git = binary / 'git'
        git.write_text('#!/bin/sh\nprintf "' + 'a' * 40 + '\\n"\n')
        git.chmod(0o755)
        self.environment = os.environ | {'PATH': str(binary) + os.pathsep + os.environ['PATH']}

    def reject(self, version='0.1.0', error=None):
        result = subprocess.run(['bash', str(self.root / '.github/scripts/release-assemble.sh')],
                                env=self.environment | {'RELEASE_VERSION': version}, capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        if error is not None:
            self.assertIn(error, result.stderr.decode())
        self.assertFalse((self.assets / 'INSTALL.md').exists())
        if (self.assets / 'BUILD.txt').exists():
            self.assertEqual((self.assets / 'BUILD.txt').read_bytes(), self.build)
        self.assertFalse((self.assets / 'SHA256SUMS').exists())

    def test_unexpected_hidden_entry_is_not_ignored(self):
        (self.assets / '.unreviewed').write_text('not a release asset')
        self.reject(error='exactly the standalone binary, VFS, materials and build receipts')

    def test_unexpected_directory_is_not_ignored(self):
        (self.assets / 'extra').mkdir()
        self.reject(error='exactly the standalone binary, VFS, materials and build receipts')

    def test_archive_symlink_cannot_read_outside_release(self):
        archive = self.assets / 'zxs-vfs-0.1.0-debian13-amd64.so'
        archive.rename(self.root / 'outside')
        archive.symlink_to(self.root / 'outside')
        self.reject(error='nonempty regular file')

    def test_asset_directory_symlink_cannot_redirect_outputs(self):
        self.assets.rename(self.root / 'outside')
        self.assets.symlink_to(self.root / 'outside', target_is_directory=True)
        self.reject(error='real directory')

    def test_invalid_version_is_rejected_before_metadata(self):
        self.reject('../outside', error='safe version filename')

    def test_missing_native_receipt_is_rejected_instead_of_reconstructed(self):
        (self.assets / 'BUILD.txt').unlink()
        self.reject(error='exactly the standalone binary, VFS, materials and build receipts')

    def test_wrong_target_is_rejected_without_overwriting_build_receipt(self):
        self.build = self.build.replace(b'x86_64-unknown-linux-gnu', b'x86_64-unknown-linux-musl')
        (self.assets / 'BUILD.txt').write_bytes(self.build)
        self.reject(error='native build target/profile identity mismatch')


if __name__ == '__main__':
    unittest.main()
