"""Unit fixtures: verified Store bytes / mountpoint selection. No test here performs a mount."""
from contextlib import contextmanager, redirect_stdout
import errno
import hashlib
import io
import os
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock
import zipfile

from zipworkspace import linux_mount
from zipworkspace.archive import _IndexedInput, bounded_directory
from zipworkspace.model import CHUNK, MAX_CD, Commit, Manifest, Rejected
from zipworkspace.snapshot_view import MAX_READ, ROOT, SnapshotView


class StreamingOutput:
    def __init__(self): self.buffer = io.BytesIO()
    def write(self, b): return self.buffer.write(b)
    def flush(self): pass


@unittest.skipUnless(sys.platform == "linux", "Linux positional snapshot reader")
class SnapshotReads(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.path = Path(tmp.name) / "renamed.zxs"
        self.body = b"A" * CHUNK + b"B" * CHUNK + b"final bytes"
        self.marker = self.write()

    def write(self, *, bad_tree=False, stream=False):
        output = StreamingOutput() if stream else self.path
        manifest = Manifest()
        with zipfile.ZipFile(output, "w") as z:
            for name, data in (("目录/大文件", self.body), ("空/", None), ("empty", b"")):
                directory = data is None
                manifest.add(name.rstrip("/"), directory, len(data or b""),
                             None if directory else hashlib.sha256(data).hexdigest())
                if not directory:
                    # Force real local ZIP64 sizes, including for a small/empty Store member.
                    with z.open(name, "w", force_zip64=True) as f: f.write(data)
                else: z.writestr(name, b"")
            marker = Commit("1"*32, "2"*32, 1, "3"*32, None, "0"*64 if bad_tree else manifest.digest())
            z.comment = marker.encode()
        if stream: self.path.write_bytes(output.buffer.getvalue())
        return marker

    def view(self):
        f = self.path.open("rb")
        self.addCleanup(f.close)
        view = SnapshotView(f, display_name=self.path.name)
        directory = view.lookup(ROOT, "目录".encode())
        return view, view.lookup(directory, "大文件".encode())

    def test_sparse_ranges_use_original_blocks_not_file_prefix_and_do_not_extract(self):
        self.write(stream=True)  # real signed, wide descriptor, after the Store body
        view, ino = self.view()
        self.assertEqual(view.commit, self.marker)
        start = view.node(ino).start
        with mock.patch("zipworkspace.snapshot_view.os.pread", wraps=os.pread) as reads:
            self.assertEqual(view.read(ino, 2*CHUNK-3, 8), self.body[2*CHUNK-3:2*CHUNK+5])
        self.assertEqual([(a.args[1], a.args[2]) for a in reads.call_args_list],
                         [(CHUNK, start+CHUNK), (11, start+2*CHUNK)])
        directory = view.lookup(ROOT, "目录".encode())
        self.assertEqual(view.lookup(directory, b".."), ROOT)
        self.assertEqual(view.lookup(directory, b"."), directory)
        self.assertEqual(view.node(view.lookup(ROOT, "空".encode())).children, [])
        self.assertEqual(view.read(view.lookup(ROOT, b"empty"), 0, 8), b"")
        self.assertEqual(view.read(ino, 10*CHUNK, 8), b"")
        self.assertEqual(list(self.path.parent.iterdir()), [self.path])

    def test_admission_tail_and_library_reads_are_capped_at_the_chosen_eof(self):
        # A normal append after observing EOF must not extend a read-all or change its allocation.
        class Recorded(io.BytesIO):
            def read(self, n=-1):
                self.calls.append(n)
                if not 0 <= n <= MAX_CD: raise AssertionError("unbounded underlying read")
                return super().read(n)
        data = self.path.read_bytes()
        stream = Recorded(data + b"later content")
        stream.calls = []
        count, central = bounded_directory(stream, len(data))
        self.assertEqual(count, 3)
        window = _IndexedInput(stream, len(data), count, central)
        window.seek(-8, os.SEEK_END)
        self.assertEqual(window.read(), data[-8:])
        self.assertEqual(stream.calls[-1], 8)
        before = list(stream.calls)
        with self.assertRaises(Rejected): window.read(MAX_CD+1)
        self.assertEqual(stream.calls, before)

    def test_library_accepts_a_genuine_large_bounded_central_table(self):
        manifest = Manifest()
        with zipfile.ZipFile(self.path, "w") as z:
            for i in range(17):
                info = zipfile.ZipInfo(f"item-{i}")
                info.comment = b"c" * 65535  # valid per-member comments; central directory > 1 MiB
                z.writestr(info, b"")
                manifest.add(info.filename, False, 0, hashlib.sha256(b"").hexdigest())
            marker = Commit("1"*32, "2"*32, 1, "3"*32, None, manifest.digest())
            z.comment = marker.encode()
        with self.path.open("rb") as doc:
            view = SnapshotView(doc, display_name=self.path.name)
            self.assertEqual(view.commit, marker)
            self.assertEqual(len(view.node(ROOT).children), 17)

    def test_no_complete_index_on_false_tree_or_invalid_member(self):
        self.write(bad_tree=True)
        with self.assertRaises(Rejected): self.view()
        self.write()
        raw = self.path.read_bytes()
        i = raw.index(b"final bytes")
        self.path.write_bytes(raw[:i] + b"X" + raw[i+1:])
        with self.assertRaises(zipfile.BadZipFile): self.view()

    def test_bad_second_block_cannot_return_good_first_part_even_with_unchanged_stat(self):
        view, ino = self.view()
        second = view.node(ino).start + CHUNK
        pread = os.pread
        def different(fd, length, pos):
            block = pread(fd, length, pos)
            return (b"!" + block[1:]) if pos == second else block
        with mock.patch("zipworkspace.snapshot_view.os.pread", side_effect=different):
            with self.assertRaises(OSError) as fail: view.read(ino, CHUNK-2, 3)
        self.assertEqual(fail.exception.errno, errno.EIO)
        with self.assertRaises(OSError): view.read(ino, len(self.body), 0)

    def test_short_positional_reads_and_interruption_are_not_zero_padded(self):
        view, ino = self.view()
        pread = os.pread
        calls = []
        def short(fd, size, pos):
            calls.append(pos)
            if len(calls) == 1: raise InterruptedError()
            return pread(fd, min(3, size), pos)
        with mock.patch("zipworkspace.snapshot_view.os.pread", side_effect=short):
            self.assertEqual(view.read(ino, 2*CHUNK, 11), b"final bytes")
        self.assertEqual(calls[0], calls[1])
        with mock.patch("zipworkspace.snapshot_view.os.pread", return_value=b""), self.assertRaises(OSError):
            view.read(ino, 2*CHUNK, 1)

    def test_mutated_or_replaced_archive_never_adopts_new_bytes(self):
        view, ino = self.view()
        with self.path.open("r+b") as w: w.truncate(1)
        with self.assertRaises(OSError) as fail: view.read(ino, 0, 0)
        self.assertEqual(fail.exception.errno, errno.EIO)
        other = self.path.with_name("new.zxs")
        other.write_bytes(b"not a ZIP")
        other.replace(self.path)
        with self.assertRaises(OSError): view.read(ino, 0, 10)

    def test_wrong_inode_kind_and_unbounded_requests(self):
        view, ino = self.view()
        for args, code in (((ROOT, 0, 1), errno.EISDIR), ((999, 0, 1), errno.ENOENT),
                           ((ino, -1, 2), errno.EINVAL), ((ino, 0, MAX_READ+1), errno.EINVAL)):
            with self.assertRaises(OSError) as err: view.read(*args)
            self.assertEqual(err.exception.errno, code)

    def test_foreground_reports_a_new_source_even_when_a_mount_id_is_reused(self):
        point = Path("/private/view here")
        old, current = "1"*32, "2"*32

        @contextmanager
        def selected(spelling, uid):
            self.assertEqual(spelling, str(point))
            yield point, (41, 42), check_point

        for token, observed_token, complete, matched in (
                (old, old, True, True), (current, old, True, False),
                (current, current, True, True), (current, current, False, False)):
            with self.subTest(request=token, observed=observed_token, complete=complete):
                output = io.StringIO()
                check_point = mock.Mock()
                rows = (b"73 5 0:8 / /private/view\\040here ro,nosuid,nodev - fuse.zip-workspace "
                        + linux_mount.mount_source(observed_token).encode("ascii") + b" ro")
                if complete: rows += b"\n"

                def unmapped_driver(view, selected_point, check, *, token, accepted):
                    self.assertEqual(selected_point, point)
                    self.assertEqual(view.commit, self.marker)  # Real immutable input/index, no kernel mount.
                    self.assertIn(f"SOURCE={linux_mount.mount_source(token)}. Not yet confirmed.", output.getvalue())
                    self.assertNotIn("ID=", output.getvalue())
                    check()
                    accepted()

                with mock.patch.dict(sys.modules, {"zipworkspace._fuse3": SimpleNamespace(serve=unmapped_driver)}), \
                     mock.patch.object(linux_mount, "_ordinary_user", return_value=1000), \
                     mock.patch.object(linux_mount, "held_point", selected), \
                     mock.patch.object(linux_mount, "mount_id", return_value=b"5") as parent, \
                     mock.patch.object(linux_mount, "new_id", return_value=token), \
                     mock.patch.object(linux_mount, "open", return_value=io.BytesIO(rows), create=True) as table, \
                     redirect_stdout(output):
                    self.assertEqual(linux_mount.main([str(self.path), str(point)]), 0)
                parent.assert_called_once_with(42)
                table.assert_called_once_with("/proc/self/mountinfo", "rb")
                check_point.assert_called_once()
                if matched:
                    self.assertIn(f"SOURCE={linux_mount.mount_source(token)}, ID=73.", output.getvalue())
                else:
                    self.assertIn("no matching mount ID has been observed", output.getvalue())
                    self.assertNotIn("ID=", output.getvalue())


@unittest.skipUnless(sys.platform == "linux", "Linux mountpoint descriptors")
class PointSelection(unittest.TestCase):
    def test_ambiguous_and_fd_spellings_do_not_open_anything(self):
        with mock.patch("zipworkspace.linux_mount.os.open", side_effect=AssertionError):
            for point in ("/", "/x/../y", "/tmp//x", "/tmp/x/", "relative", "/dev/fd/7", "/proc/self/fd/9"):
                with self.subTest(point=point), self.assertRaises(Rejected):
                    with linux_mount.held_point(point, os.getuid()): pass

    def test_held_private_empty_point_is_rechecked_before_libfuse_gets_the_name(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            parent = root/"parent"; parent.mkdir(mode=0o700)
            leaf = parent/"view"; leaf.mkdir(mode=0o700)
            real_open = os.open
            def enter_fixture(path, flags, *args, **kwargs):
                # Root only is redirected to the disposable tree; checks below use real directory FDs.
                return real_open(root if path == "/" else path, flags, *args, **kwargs)
            with mock.patch.object(linux_mount.os, "open", side_effect=enter_fixture), \
                    mock.patch.object(linux_mount, "local_mount", return_value=b"test"):
                def mounted_leaf(fd):
                    return b"other" if os.fstat(fd).st_ino == leaf.stat().st_ino else b"original"
                with mock.patch.object(linux_mount, "mount_id", side_effect=mounted_leaf), self.assertRaises(Rejected):
                    with linux_mount.held_point("/parent/view", os.getuid()): pass
                with linux_mount.held_point("/parent/view", os.getuid()) as (_, _, check):
                    check()
                    (leaf/"occupied").touch()
                    with self.assertRaises(Rejected): check()
                    leaf.rename(parent/"original")
                    leaf.mkdir(mode=0o700)
                    with self.assertRaises(Rejected): check()
                leaf.rename(parent/"other")
                leaf.symlink_to(parent/"other", target_is_directory=True)
                with self.assertRaises(OSError):
                    with linux_mount.held_point("/parent/view", os.getuid()): pass


try:
    from zipworkspace import _fuse3 as driver
except (ImportError, OSError):
    driver = None


@unittest.skipUnless(sys.platform == "linux" and driver is not None, "optional Linux pyfuse3/Trio")
class AdapterContract(unittest.TestCase):
    def test_actual_adapter_callbacks_and_directory_resume_without_kernel_mount(self):
        case = SnapshotReads()
        case.setUp()
        self.addCleanup(case.doCleanups)
        view, ino = case.view()
        ops = driver.Operations(view)
        async def requests():
            info = await ops.open(ino, os.O_RDONLY, None)
            self.assertTrue(info.direct_io)
            self.assertFalse(info.keep_cache)
            self.assertEqual(await ops.read(info.fh, 2*CHUNK+1, 2), b"in")
            for name in ("open", "write", "unlink"):
                with self.assertRaises(driver.pyfuse3.FUSEError) as err:
                    if name == "open": await ops.open(ino, os.O_RDWR, None)
                    else: await getattr(ops, name)(ino, b"ignored", None)
                self.assertEqual(err.exception.errno, errno.EROFS)
            found = []
            def first(token, name, attr, next_id):
                if len(found) == 1: return False
                found.append((name, next_id)); return True
            with mock.patch.object(driver.pyfuse3, "readdir_reply", side_effect=first):
                await ops.readdir(ROOT, 0, None)
            rest = []
            with mock.patch.object(driver.pyfuse3, "readdir_reply",
                                   side_effect=lambda t,n,a,i: (rest.append((n,i)) or True)):
                await ops.readdir(ROOT, found[-1][1], None)
            self.assertEqual([x[0] for x in found+rest], [x[0] for x in view.node(ROOT).children])
        driver.trio.run(requests)

    def test_mount_is_explicit_read_only_and_never_cleans_an_unknown_namespace(self):
        case = SnapshotReads()
        case.setUp()
        self.addCleanup(case.doCleanups)
        view, _ = case.view()
        token, accepted = "6"*32, mock.Mock()
        with mock.patch.object(driver.pyfuse3, "init") as init, mock.patch.object(driver.pyfuse3, "close") as close, \
                mock.patch.object(driver.trio, "run", side_effect=RuntimeError("disconnected")):
            with self.assertRaises(Rejected):
                driver.serve(view, Path("/unused"), mock.Mock(side_effect=Rejected("replaced")),
                             token=token, accepted=accepted)
            init.assert_not_called(); close.assert_not_called(); accepted.assert_not_called()
            with self.assertRaises(RuntimeError):
                driver.serve(view, Path("/unused"), lambda: None, token=token, accepted=accepted)
            self.assertEqual(init.call_args.args[2], {"ro","nodev","nosuid","noexec","default_permissions",
                                                      "fsname=" + linux_mount.mount_source(token)})
            accepted.assert_called_once()
            close.assert_called_once_with(unmount=False)
