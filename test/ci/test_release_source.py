"""Exact source closure uses tiny Git fixtures, never a compiler or live release."""
import gzip
import importlib.util
import io
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('release_source', ROOT / '.github/scripts/release-source.py')
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class ReleaseSource(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='zxs source objects ')
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.git('init', '-q')
        for name, data in {'.gitattributes': b'private export-ignore\nversion.txt export-subst\n',
                           'private/source.rs': b'original source\n',
                           'version.txt': b'$Format:%H$\n',
                           'compliance/LICENSE': b'original license\n',
                           'executable': b'not run\n', 'empty': b'',
                           'unicode/源.rs': b'unicode path\n'}.items():
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        (self.root / 'executable').chmod(0o755)
        self.commit_source()

    def git(self, *args):
        return subprocess.check_output(['git', '-C', str(self.root), *args], text=True).strip()

    def commit_source(self):
        self.git('add', '.')
        self.git('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                 'commit', '-qm', 'source fixture')
        self.commit, self.tree = self.git('rev-parse', 'HEAD'), self.git('rev-parse', 'HEAD^{tree}')

    def prepare(self, name='source.tar.gz'):
        path = self.root / name
        receipt = MODULE.prepare(self.root, path, self.commit, self.tree)
        return path, receipt

    def rewritten(self, source, edit):
        result = self.root / 'changed.tar.gz'
        with tarfile.open(source, 'r:gz') as archive:
            items = [(entry, archive.extractfile(entry).read()) for entry in archive]
        items = edit(items)
        with tarfile.open(result, 'w:gz', format=tarfile.PAX_FORMAT) as archive:
            for entry, data in items:
                entry.size = len(data)
                archive.addfile(entry, io.BytesIO(data))
        return result

    def verify(self, path):
        return MODULE.verify(path, self.commit, self.tree)

    def test_complete_deterministic_objects_ignore_checkout_and_export_filters(self):
        (self.root / 'private/source.rs').write_bytes(b'dirty checkout is not source authority')
        (self.root / 'untracked').write_bytes(b'not part of commit')
        first, receipt = self.prepare()
        second, repeated = self.prepare('second.tar.gz')
        self.assertEqual(first.read_bytes(), second.read_bytes())
        self.assertEqual(receipt, repeated)
        with tarfile.open(first) as archive:
            self.assertEqual(archive.extractfile('source/private/source.rs').read(), b'original source\n')
            self.assertEqual(archive.extractfile('source/version.txt').read(), b'$Format:%H$\n')
            self.assertEqual(archive.getmember('source/executable').mode, 0o755)
            self.assertEqual(archive.getmember('source/compliance/LICENSE').mode, 0o644)
            self.assertNotIn('source/untracked', archive.getnames())
        self.assertEqual(self.verify(first)['file_count'], 7)

    def test_modified_removed_or_repermissioned_source_cannot_match_tree(self):
        path, _ = self.prepare()
        def modified(items):
            return [(entry, b'changed' if entry.name == 'source/empty' else data) for entry, data in items]
        def removed(items):
            return [(entry, data) for entry, data in items if entry.name != 'source/empty']
        def permissions(items):
            next(entry for entry, _ in items if entry.name == 'source/executable').mode = 0o644
            return items
        for edit in (modified, removed, permissions):
            with self.subTest(edit=edit.__name__), self.assertRaisesRegex(ValueError, 'complete requested Git tree'):
                self.verify(self.rewritten(path, edit))

    def test_commit_or_tree_cannot_be_relabelled(self):
        path, _ = self.prepare()
        for commit, tree in (('a' * 40, self.tree), (self.commit, 'a' * 40)):
            with self.subTest(commit=commit, tree=tree), self.assertRaisesRegex(ValueError, 'commit/tree'):
                MODULE.verify(path, commit, tree)

    def test_missing_commit_and_duplicate_source_are_rejected(self):
        path, _ = self.prepare()
        for edit in (lambda items: items[1:], lambda items: items + [items[-1]]):
            with self.assertRaises(ValueError):
                self.verify(self.rewritten(path, edit))

    def test_unsafe_paths_links_and_special_entries_are_rejected_without_extraction(self):
        path, _ = self.prepare()
        for name, kind in (('source/../escape', tarfile.REGTYPE), ('other', tarfile.REGTYPE),
                           ('source/.git/config', tarfile.REGTYPE), ('source/link', tarfile.SYMTYPE),
                           ('source/hardlink', tarfile.LNKTYPE), ('source/fifo', tarfile.FIFOTYPE)):
            def edit(items):
                entry = tarfile.TarInfo(name)
                entry.type, entry.mode, entry.linkname = kind, 0o644, '../../escape'
                return items + [(entry, b'')]
            with self.subTest(name=name), self.assertRaises(ValueError):
                self.verify(self.rewritten(path, edit))
        self.assertFalse((self.root.parent / 'escape').exists())

    def test_committed_symlinks_fail_before_creating_output(self):
        (self.root / 'link').symlink_to('private/source.rs')
        self.commit_source()
        with self.assertRaisesRegex(ValueError, 'regular Git blobs'):
            self.prepare()
        self.assertFalse((self.root / 'source.tar.gz').exists())

    def test_existing_output_and_symlink_are_never_overwritten(self):
        path, _ = self.prepare()
        original = path.read_bytes()
        with self.assertRaises(FileExistsError):
            self.prepare()
        (self.root / 'link-output').symlink_to(path)
        with self.assertRaises(FileExistsError):
            self.prepare('link-output')
        self.assertEqual(path.read_bytes(), original)

    def test_verifier_rejects_link_or_fifo_before_reading_archive_bytes(self):
        path, _ = self.prepare()
        linked = self.root / 'linked'
        linked.symlink_to(path)
        with self.assertRaises(OSError):
            self.verify(linked)
        fifo = self.root / 'fifo'
        os.mkfifo(fifo)
        with self.assertRaisesRegex(ValueError, 'regular file'):
            self.verify(fifo)

    def test_source_count_and_byte_limits_cover_production_and_readback(self):
        path, _ = self.prepare()
        for name, value in (('MAX_FILES', 1), ('MAX_FILE_BYTES', 1), ('MAX_TOTAL_BYTES', 1),
                            ('MAX_COMMIT_BYTES', 1), ('MAX_INVENTORY_BYTES', 1)):
            with self.subTest(limit=name), patch.object(MODULE, name, value):
                with self.assertRaises(ValueError):
                    self.prepare('limited.tar.gz')
                self.assertFalse((self.root / 'limited.tar.gz').exists())
                if name != 'MAX_INVENTORY_BYTES':
                    with self.assertRaises(ValueError):
                        self.verify(path)
        with patch.object(MODULE, 'MAX_TAR_BYTES', 16), self.assertRaises(ValueError):
            self.verify(path)

    def test_gzip_corruption_is_not_hidden_after_tar_end(self):
        path, _ = self.prepare()
        data = bytearray(path.read_bytes())
        data[-8] ^= 1
        path.write_bytes(data)
        with self.assertRaises((gzip.BadGzipFile, tarfile.ReadError)):
            self.verify(path)

    def test_appended_tar_cannot_hide_an_extra_source_after_end_markers(self):
        path, _ = self.prepare()
        extra = io.BytesIO()
        with tarfile.open(fileobj=extra, mode='w') as archive:
            MODULE.add_bytes(archive, 'source/hidden', 0o644, b'extra')
        path.write_bytes(path.read_bytes() + gzip.compress(extra.getvalue()))
        with self.assertRaisesRegex(ValueError, 'complete requested Git tree'):
            self.verify(path)

    def test_failed_readback_does_not_leave_a_success_looking_archive(self):
        with patch.object(MODULE, 'verify', side_effect=ValueError('readback failed')):
            with self.assertRaisesRegex(ValueError, 'readback failed'):
                self.prepare()
        self.assertFalse((self.root / 'source.tar.gz').exists())

    def test_git_replace_objects_do_not_relabel_the_original_source(self):
        original_commit, original_tree = self.commit, self.tree
        (self.root / 'empty').write_bytes(b'replacement')
        self.commit_source()
        self.git('replace', original_commit, self.commit)
        self.commit, self.tree = original_commit, original_tree
        path, _ = self.prepare()
        with tarfile.open(path) as archive:
            self.assertEqual(archive.extractfile('source/empty').read(), b'')


if __name__ == '__main__':
    unittest.main()
