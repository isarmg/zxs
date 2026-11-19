"""Fixed macOS read callbacks, no kernel volume. Only the explicitly opted-in test loads libfuse2."""
import ctypes as C
from dataclasses import replace
import errno
from functools import partial
import hashlib
from importlib.metadata import version
import os
from pathlib import Path
import stat
import sys
import tempfile
from types import SimpleNamespace
from unicodedata import normalize
import unittest
from unittest.mock import patch
import zipfile

from zipworkspace.desktop.selection import Selection
from zipworkspace.macos_snapshot import selected_callbacks
from zipworkspace.model import CHUNK, Commit, Manifest, Rejected
from zipworkspace.refs import Node

SDK_PATH = "/usr/local/lib/libfuse.2.dylib"


@unittest.skipUnless(sys.platform == "darwin", "a normal macOS user and a resident local temporary directory")
class MacCallbacks(unittest.TestCase):
    def setUp(self):
        private = tempfile.TemporaryDirectory()
        self.addCleanup(private.cleanup)
        self.path = Path(private.name)/"example.zxs"
        self.body = b"a" * CHUNK + b"BCDEF"
        manifest = Manifest()
        with zipfile.ZipFile(self.path, "w") as archive:
            for name, data in (("Café/한", self.body), ("Café/" + "한"*80, b"x"), ("empty/", None), ("zero", b"")):
                archive.writestr(name, data or b"")
                manifest.add(name.rstrip("/"), data is None, len(data or b""),
                             None if data is None else hashlib.sha256(data).hexdigest())
            self.commit = Commit("1"*32, "2"*32, 1, "3"*32, None, manifest.digest())
            archive.comment = self.commit.encode()
        self.selected = Selection.open(self.path, retain=True)
        self.addCleanup(self.selected.release_display)

    def opened(self, ops, path):
        fi = SimpleNamespace(flags=os.O_RDONLY)
        self.assertEqual(ops.open(path, fi), 0)
        return fi

    def test_nfd_nfc_original_inode_case_and_no_synthetic_appledouble(self):
        with selected_callbacks(self.selected) as ops:
            expected = ops.getattr("/Café/한")
            self.assertEqual(ops.getattr(normalize("NFD", "/Café/한")), expected)
            self.assertEqual(expected["st_size"], len(self.body))
            self.assertTrue(stat.S_ISREG(expected["st_mode"]))
            directory = ops.opendir("/Cafe\u0301")
            rows = list(ops.readdir_with_offset(None, 0, directory))
            long_name = normalize("NFD", "한"*80)  # 240 NFC bytes, 720 Finder bytes
            self.assertEqual([name for name, _, _ in rows], [".", "..", normalize("NFD", "한"), long_name])
            self.assertEqual(rows[2][1]["st_ino"], expected["st_ino"])
            self.assertEqual(ops.getattr("/Café/" + long_name), rows[3][1])
            for wrong, code in (("/café/한", errno.ENOENT), ("/._Café", errno.ENOENT),
                                ("/\udcff", errno.EILSEQ), ("/../zero", errno.EINVAL)):
                with self.subTest(path=wrong), self.assertRaises(OSError) as err: ops.getattr(wrong)
                self.assertEqual(err.exception.errno, code)
            empty = ops.opendir("/empty")
            self.assertEqual([s for s, _, _ in ops.readdir_with_offset("/empty", 0, empty)], [".", ".."])

    def test_missing_retention_wrong_listed_raw_and_no_shared_display_cursor(self):
        with self.assertRaises(Rejected):
            with selected_callbacks(Selection.open(self.path)): self.fail("no retained original")
        expected = Node(self.commit, self.path, "0"*64, self.selected.observed)
        with self.assertRaises(Rejected):
            with selected_callbacks(replace(self.selected, exact=expected)): self.fail("wrong listed ZIP")
        handle = self.selected.held.fileno()
        os.lseek(handle, 7, os.SEEK_SET)
        with patch("os.open", side_effect=AssertionError("no reopen of the archive name")):
            with selected_callbacks(self.selected) as ops:
                fi = self.opened(ops, "/Café/한")
                self.assertEqual(ops.read(None, 4, CHUNK-1, fi), b"aBCD")
                self.assertEqual((fi.direct_io, fi.keep_cache), (1, 0))
                self.assertEqual(os.lseek(handle, 0, os.SEEK_CUR), 7)
                self.selected.release_display()  # provider has the original inode, not a borrowed integer
                self.assertEqual(ops.read("/Café/한", 9, len(self.body)-1, fi), b"F")
            with self.assertRaises(OSError) as err: ops.getattr("/")
            self.assertEqual(err.exception.errno, errno.EIO)

    def test_changed_or_short_second_block_returns_no_prefix_and_cannot_revive_at_eof(self):
        for short in (False, True):
            with selected_callbacks(self.selected) as ops:
                fi = self.opened(ops, "/Café/한")
                second = ops.view.node(fi.fh).start + CHUNK
                real = os.pread
                def altered(fd, size, pos):
                    data = real(fd, size, pos)
                    return (b"" if short else b"!" + data[1:]) if pos == second else data
                with patch("os.pread", side_effect=altered), self.assertRaises(OSError) as err:
                    ops.read(None, 2, CHUNK-1, fi)
                self.assertEqual(err.exception.errno, errno.EIO)
                with self.assertRaises(OSError) as err: ops.read(None, 0, len(self.body), fi)
                self.assertEqual(err.exception.errno, errno.EIO)

    def test_refuses_writes_file_execute_changed_file_and_wrong_handle(self):
        with selected_callbacks(self.selected) as ops:
            for opname in ("write", "rename", "utimens", "setxattr", "truncate", "create"):
                with self.subTest(op=opname), self.assertRaises(OSError) as err: getattr(ops, opname)("/zero", 0)
                self.assertEqual(err.exception.errno, errno.EROFS)
            with self.assertRaises(OSError): ops.open("/zero", SimpleNamespace(flags=os.O_RDWR))
            with self.assertRaises(OSError): ops.access("/zero", os.X_OK)
            self.assertEqual(ops.access("/empty", os.X_OK), 0)
            fi = self.opened(ops, "/zero")
            self.assertEqual(ops.read(None, 8, 0, fi), b"")
            with self.assertRaises(OSError) as err: ops.read("/Café/한", 1, 0, fi)
            self.assertEqual(err.exception.errno, errno.EBADF)
            with self.path.open("ab") as changed: changed.write(b"later")
            with self.assertRaises(OSError) as err: ops.read(None, 1, 0, fi)
            self.assertEqual(err.exception.errno, errno.EIO)

    def test_two_directory_pages_use_last_accepted_cookie(self):
        with selected_callbacks(self.selected) as ops:
            root = ops.opendir("/")
            first = list(ops.readdir_with_offset("/", 0, root))[:3]
            second = list(ops.readdir_with_offset(None, first[-1][2], root))
            self.assertEqual([x[0] for x in first+second], [".", "..", "Cafe\u0301", "empty", "zero"])
            self.assertEqual([x[2] for x in first+second], list(range(1, 6)))
            self.assertEqual(list(ops.readdir_with_offset(None, 5, root)), [])
            for offset in (-1, 6):
                with self.assertRaises(OSError): list(ops.readdir_with_offset(None, offset, root))

    @unittest.skipUnless(os.environ.get("ZIP_WORKSPACE_MACFUSE_ABI") == "CHECK_INSTALLED_BINDING_NO_MOUNT",
                         "opt in separately on a trusted host; default tests never import the native binding")
    def test_installed_mfusepy_libfuse2_raw_callbacks_and_full_native_filler(self):
        # The operator has independently verified the official macFUSE installation, this binding
        # and the interpreter. A pathname and a version string are not a code-signature check.
        self.assertEqual(os.environ.get("FUSE_LIBRARY_PATH"), SDK_PATH)
        self.assertNotIn("mfusepy", sys.modules)
        self.assertEqual(version("mfusepy"), "3.1.1")
        import mfusepy as sdk
        self.assertEqual((sdk._system, sdk.fuse_version_major, sdk._libfuse._name), ("Darwin", 2, SDK_PATH))
        # _wrapper handles BaseException by calling the real fuse_exit(). There is no session
        # in this fixture, so the target of that one terminal dispatch must never be native.
        terminate = self.enterContext(patch.object(sdk, "fuse_exit"))
        spec = {field[0]: field[1] for field in sdk.fuse_operations._fields_}
        with selected_callbacks(self.selected) as ops:
            adapter = object.__new__(sdk.FUSE)  # __init__ mounts; deliberately do not call it
            adapter.operations, adapter.raw_fi, adapter.use_ns = ops, True, True
            adapter.encoding, adapter.errors = "utf-8", "surrogateescape"
            def callback(name, method=None):
                return spec[name](partial(adapter._wrapper, getattr(adapter, method or name)))
            fi = sdk.fuse_file_info()
            fip = C.pointer(fi)
            name = "/Cafe\u0301/\u1112\u1161\u11ab".encode()
            self.assertEqual(callback("open")(name, fip), 0)
            out = (C.c_byte * 6)()
            self.assertEqual(callback("read")(name, out, 6, CHUNK-1, fip), 6)
            self.assertEqual(bytes(out), b"aBCDEF")
            self.assertEqual(callback("release")(name, fip), 0)
            buf = sdk.c_stat()
            attrs = callback("getattr", "getattr_fuse_2")
            self.assertEqual(attrs(name, C.pointer(buf)), 0)
            self.assertEqual(buf.st_ino, fi.fh)
            self.assertEqual(attrs(b"/\xff", C.pointer(buf)), -errno.EILSEQ)
            self.assertEqual(callback("opendir")(b"/", fip), 0)
            list_page = callback("readdir", "readdir_fuse_2")
            def page(cookie, limit):
                accepted, rejected = [], []
                def fill(_, leaf, attrs, next_cookie):
                    record = (leaf, attrs.contents.st_ino, next_cookie)
                    if len(accepted) == limit: rejected.append(record); return 1
                    accepted.append(record); return 0
                filler = spec["readdir"]._argtypes_[2](fill)
                self.assertEqual(list_page(b"/", None, filler, cookie, fip), 0)
                return accepted, rejected
            first, full = page(0, 2)
            rest, end = page(first[-1][2], 8)
            self.assertEqual(full, [rest[0]])
            self.assertEqual([x[0] for x in first+rest], [b".", b"..", "Cafe\u0301".encode(), b"empty", b"zero"])
            self.assertEqual(end, [])
            self.assertEqual(callback("releasedir")(b"/", fip), 0)
            terminate.assert_not_called()
            with patch.object(ops, "getattr", side_effect=KeyboardInterrupt()):
                self.assertEqual(attrs(name, C.pointer(buf)), -errno.EFAULT)
            terminate.assert_called_once_with()
