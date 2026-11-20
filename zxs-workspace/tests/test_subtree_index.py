"""The selected logical subtree is reconstructed before creating a private writer."""
import hashlib
import io
import unittest
import zipfile

from zipworkspace.archive import stored_subtree
from zipworkspace.model import Commit, Rejected


class SelectedTreeIndex(unittest.TestCase):
    marker = Commit("1"*32, "2"*32, 1, "3"*32, None, "4"*64)

    def archive(self, records):
        content = io.BytesIO()
        with zipfile.ZipFile(content, "w", compression=zipfile.ZIP_STORED) as archive:
            for name, body in records: archive.writestr(name, body)
            archive.comment = self.marker.encode()
        return content, content.tell()

    def test_only_exact_prefix_and_real_subtree_records_not_another_similar_name(self):
        body = b"valid member"
        content, end = self.archive((("root/sub/data", body), ("root/empty/", b""), ("root-copy/foreign", b"x")))
        rows = {"sub": (True, 0, None), "sub/data": (False, len(body), hashlib.sha256(body).hexdigest()),
                "empty": (True, 0, None)}
        with stored_subtree(content, end, self.marker, "root", rows) as (archive, files):
            self.assertEqual([path for path, _ in files], ["sub/data"])
            self.assertEqual(archive.read(files[0][1]), body)

    def test_empty_folder_is_only_real_if_its_record_exists(self):
        content, end = self.archive((("chosen/", b""), ("chosen-copy/nested", b"x")))
        with stored_subtree(content, end, self.marker, "chosen", {}) as (_, files):
            self.assertEqual(files, [])
        with self.assertRaisesRegex(Rejected, "missing an original"):
            with stored_subtree(content, end, self.marker, "chosen-copy-too", {}):
                self.fail("the suffix may not stand in for the actual empty root")

    def test_missing_empty_directory_or_file_shadow_cannot_be_prepared(self):
        records = (("root/visible", b"B"),)
        rows = {"visible": (False, 1, hashlib.sha256(b"B").hexdigest()), "empty": (True, 0, None)}
        content, end = self.archive(records)
        with self.assertRaisesRegex(Rejected, "missing an original"):
            with stored_subtree(content, end, self.marker, "root", rows):
                self.fail("do not silently add a missing empty entry")
        content, end = self.archive(records + (("root", b"not a folder"), ("root/empty/", b"")))
        with self.assertRaisesRegex(Rejected, "directory is a file"):
            with stored_subtree(content, end, self.marker, "root", rows):
                self.fail("file shadows the selected directory")
