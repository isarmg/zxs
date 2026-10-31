"""Build-time embedding uses the production receipt without depending on native output."""
import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from web_dist_fixture import prepare

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('embed_web_assets', ROOT / '.github/scripts/embed-web-assets.py')
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class EmbeddedWeb(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name) / 'source'
        self.root.mkdir()
        prepare(self.root)
        (self.root / '.gitignore').write_text('build/\ntarget/\n')
        self.out = Path(self.temporary.name) / 'out'
        self.out.mkdir()
        self.git('init', '-q')
        self.git('add', '.')
        self.git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'fixture')
        self.env = {'ZXS_EMBED_WEB_COMMIT': self.git('rev-parse', 'HEAD'),
                    'ZXS_EMBED_WEB_TREE': self.git('rev-parse', 'HEAD^{tree}')}

    def git(self, *args):
        return subprocess.check_output(['git', '-C', str(self.root), *args], text=True).strip()

    def generate(self):
        with patch.dict(os.environ, self.env):
            MODULE.generate(self.root, self.out)

    def test_snapshot_is_exact_source_bound_and_independent_of_later_asset_changes(self):
        for name in ('zxs', 'zxs-rescue'):
            (self.root / 'target/native/x86_64-unknown-linux-gnu/release' / name).unlink()
        self.generate()
        generated = (self.out / 'embedded_web.rs').read_text()
        self.assertIn(self.env['ZXS_EMBED_WEB_COMMIT'], generated)
        self.assertIn(self.env['ZXS_EMBED_WEB_TREE'], generated)
        snapshots = {path.name: path.read_bytes() for path in (self.out / 'web-assets').iterdir()}
        (self.root / 'build/web/index.html').write_bytes(b'later')
        self.assertEqual(snapshots, {path.name: path.read_bytes() for path in (self.out / 'web-assets').iterdir()})

    def test_foreign_head_tree_and_dirty_source_are_rejected(self):
        for key in self.env:
            original = self.env[key]
            self.env[key] = '0' * 40
            with self.assertRaisesRegex(ValueError, 'commit/tree mismatch'):
                self.generate()
            self.env[key] = original
        (self.root / 'README.md').write_text('dirty')
        with self.assertRaisesRegex(ValueError, 'clean source'):
            self.generate()

    def test_changed_committed_inputs_cannot_relabel_an_old_web_build(self):
        (self.root / 'Cargo.lock').write_text('changed lock')
        self.git('add', 'Cargo.lock')
        self.git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'changed')
        self.env = {'ZXS_EMBED_WEB_COMMIT': self.git('rev-parse', 'HEAD'),
                    'ZXS_EMBED_WEB_TREE': self.git('rev-parse', 'HEAD^{tree}')}
        with self.assertRaisesRegex(ValueError, 'source input'):
            self.generate()

    def test_tampered_missing_extra_and_hardlinked_outputs_are_rejected(self):
        path = self.root / 'build/web/index.html'
        original = path.read_bytes()
        path.write_bytes(b'tampered')
        with self.assertRaisesRegex(ValueError, 'checksum mismatch'): self.generate()
        path.unlink()
        with self.assertRaises(FileNotFoundError): self.generate()
        path.write_bytes(original)
        extra = self.root / 'build/web/unknown'
        extra.write_bytes(b'extra')
        with self.assertRaisesRegex(ValueError, 'inventory'): self.generate()
        extra.unlink()
        os.link(path, Path(self.temporary.name) / 'hardlink')
        with self.assertRaisesRegex(ValueError, 'link/size'): self.generate()

    def test_link_and_duplicate_manifest_fields_are_rejected(self):
        path = self.root / 'build/web/index.html'
        path.unlink()
        path.symlink_to(self.root / 'README.md')
        with self.assertRaisesRegex(ValueError, 'regular file'): self.generate()
        path.unlink()
        path.write_text('unused')
        (self.root / 'build/web/build-manifest.json').write_text('{"runtime":1,"runtime":2}')
        with self.assertRaisesRegex(ValueError, 'duplicate'): self.generate()

    def test_count_per_file_and_total_byte_budgets_are_enforced(self):
        for name in ('MAX_FILES', 'MAX_FILE_BYTES', 'MAX_TOTAL_BYTES'):
            with patch.object(MODULE, name, 1), self.assertRaisesRegex(ValueError, 'limit'):
                self.generate()


if __name__ == '__main__':
    unittest.main()
