"""A fresh synthetic CLI STATE. No test switches an unqualified filesystem into a supported one."""
from contextlib import closing, contextmanager, redirect_stdout
import errno
import hashlib
import io
import os
from pathlib import Path
import sqlite3
import sys
import tempfile
import unittest
from unittest.mock import patch
import zipfile

from zipworkspace import cli, opening, private_stage as staging, worktree
from zipworkspace.capacity import describe
from zipworkspace.model import Commit, Manifest, Rejected


@unittest.skipUnless(sys.platform.startswith("linux"), "protected staging uses Linux mount evidence")
class PrivateCheckoutStage(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory(); self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.private, self.out, self.store = [self.root / n for n in ("private", "worktrees", "store")]
        for path in (self.private, self.out, self.store): path.mkdir(mode=0o700)
        self.state = self.private / "instance"; self.archive = self.root / "picked.zxs"
        tree = Manifest(); entries = (("nested/a", b"selected bytes"), ("zero", b""), ("empty/", b""))
        for name, data in entries:
            directory = name.endswith("/")
            tree.add(name.rstrip("/"), directory, len(data), None if directory else hashlib.sha256(data).hexdigest())
        self.marker = Commit("1"*32, "2"*32, 1, "3"*32, None, tree.digest())
        with zipfile.ZipFile(self.archive, "x", compression=zipfile.ZIP_STORED) as z:
            for name, data in entries: z.writestr(name, data)
            z.comment = self.marker.encode()

    def qualified(self):
        a = os.open(self.private, os.O_DIRECTORY); b = os.open(self.out, os.O_DIRECTORY)
        try:
            try: staging.same_mount(a, b)
            except (OSError, Rejected) as e: self.skipTest("requires a real admitted mount: " + str(e))
        finally: os.close(b); os.close(a)

    def begin(self, name="Chosen", *, automatic=False):
        return opening.begin(self.archive, self.out, name, self.store, self.state, automatic=automatic, private_stage=True)

    def prepare(self, name="Chosen", *, automatic=False):
        with patch.object(opening, "finish", side_effect=InterruptedError("STATE installed")):
            with self.assertRaisesRegex(InterruptedError, "STATE installed"): self.begin(name, automatic=automatic)
        return opening.status(self.state)["operation"]

    def test_complete_tree_and_only_one_prepared_authority_before_any_public_name(self):
        self.qualified()
        operation = self.prepare(staging.LEAF)  # Same literal in two distinct held parents.
        self.assertEqual(list(self.out.iterdir()), [])
        self.assertEqual({p.name for p in self.state.iterdir()}, {"original.zxs", "opening.sqlite", staging.LEAF})
        stage = self.state / staging.LEAF; inode = stage.stat().st_ino
        self.assertTrue((stage / "empty").is_dir()); self.assertEqual((stage / "zero").read_bytes(), b"")
        self.assertEqual((stage / "nested/a").read_bytes(), b"selected bytes")
        with closing(sqlite3.connect((self.state / "opening.sqlite").as_uri() + "?mode=ro", uri=True)) as con:
            self.assertEqual(con.execute("SELECT name FROM sqlite_master WHERE type='table'").fetchall(), [("opening",)])
            self.assertEqual(con.execute("SELECT operation,stage,stage_ino,private_inode,done FROM opening").fetchone(),
                             (operation, staging.LEAF, str(inode), str(self.state.stat().st_ino), 0))
        self.archive.rename(self.root / "offline.zxs")
        with redirect_stdout(io.StringIO()): self.assertEqual(cli.main(["resume-checkout", str(self.state), operation]), 0)
        target = self.out / staging.LEAF
        self.assertEqual(target.stat().st_ino, inode)
        self.assertEqual((target / "nested/a").read_bytes(), b"selected bytes"); self.assertFalse(stage.exists())
        self.assertEqual(worktree.status(self.state)["base_operation"], self.marker.operation)

    def test_private_cli_selects_a_new_real_name_without_replacing_a_concurrent_occupant(self):
        self.qualified()
        taken = self.out / "Project"; taken.mkdir(); (taken / "mine").write_bytes(b"original")
        with redirect_stdout(io.StringIO()):
            result = cli.main(["checkout-auto", str(self.archive), str(self.out), str(self.store),
                               str(self.state), "Project", "--private-stage"])
        self.assertEqual(result, 0)
        self.assertEqual((taken / "mine").read_bytes(), b"original")
        self.assertEqual((self.out / "Project (2)/nested/a").read_bytes(), b"selected bytes")
        self.assertEqual({p.name for p in self.out.iterdir()}, {"Project", "Project (2)"})

    def test_each_original_parent_barrier_is_required_again_after_the_actual_cross_parent_move(self):
        self.qualified()
        for failed in ("private", "target"):
            with self.subTest(failed=failed):
                self.state = self.private / failed; dest = self.out / failed
                op = self.prepare(failed); source = self.state / staging.LEAF
                inode = source.stat().st_ino; native, sync = opening.rename_new_at, os.fsync
                moved, barriers = [], set()
                def rename(fd, src, dst, *, expected_source, target_fd):
                    self.assertTrue(os.path.samestat(os.fstat(fd), self.state.stat()))
                    self.assertTrue(os.path.samestat(os.fstat(target_fd), self.out.stat()))
                    native(fd, src, dst, expected_source=expected_source, target_fd=target_fd); moved.append(True)
                def barrier(fd):
                    label = ("target" if os.path.samestat(os.fstat(fd), self.out.stat()) else
                             "private" if os.path.samestat(os.fstat(fd), self.state.stat()) else None)
                    if label: barriers.add(label)
                    if moved and label == failed: raise OSError(errno.ENOSPC, "original parent sync denied")
                    return sync(fd)
                with patch.object(opening, "rename_new_at", side_effect=rename), patch.object(opening.os, "fsync", side_effect=barrier):
                    with self.assertRaisesRegex(OSError, "parent sync denied") as err: opening.finish(self.state, op)
                role = "STATE " + str(self.state) if failed == "private" else "工作目录父 " + str(self.out)
                self.assertIn(role, describe(err.exception)); self.assertIn(op, describe(err.exception))
                self.assertIn("ENOSPC", describe(err.exception))
                self.assertEqual(moved, [True]); self.assertEqual(dest.stat().st_ino, inode)
                self.assertFalse(source.exists()); self.assertFalse((self.state / "worktree.sqlite").exists())
                self.assertEqual(opening.status(self.state)["recorded_stage"], "verified_stage")
                moved.clear(); barriers.clear()
                with patch.object(opening, "rename_new_at", side_effect=AssertionError("no second cross-parent move")), \
                     patch.object(opening.os, "fsync", side_effect=barrier):
                    self.assertEqual(opening.finish(self.state, op), (dest, self.marker))
                self.assertEqual(barriers, {"private", "target"}); self.assertEqual(dest.stat().st_ino, inode)

    @contextmanager
    def another_named_mount(self, name):
        # Simulate only the kernel mount identity returned for a named O_PATH
        # reopen. The inode, the original held FDs and the actual I/O stay real.
        native_open, native_mount = os.open, staging.mount_id
        named = set()

        def opened(path, flags, *args, dir_fd=None):
            fd = native_open(path, flags, *args, dir_fd=dir_fd)
            if (dir_fd is not None and path == name.name and flags & os.O_PATH
                    and os.path.samestat(os.fstat(dir_fd), name.parent.stat())):
                named.add(fd)
            return fd

        def mount(fd):
            if fd in named:
                self.assertTrue(os.path.samestat(os.fstat(fd), name.stat()))
                return native_mount(fd) + b"000"
            return native_mount(fd)

        with patch.object(os, "supports_dir_fd", os.supports_dir_fd | {opened}), \
             patch.object(os, "open", new=opened), patch.object(staging, "mount_id", side_effect=mount):
            yield named

    def test_same_inode_at_a_different_mount_cannot_install_the_prepared_worktree(self):
        self.qualified(); operation = self.prepare(); source = self.state/staging.LEAF
        for leaf in (self.state, source):
            with self.subTest(leaf=leaf), self.another_named_mount(leaf) as checked:
                with patch.object(opening, "rename_new_at", side_effect=AssertionError("no worktree move")), \
                     self.assertRaisesRegex(Rejected, "original mount"):
                    opening.finish(self.state, operation)
                self.assertTrue(checked)
        self.assertTrue(source.is_dir()); self.assertFalse((self.out/"Chosen").exists())
        self.assertEqual(opening.status(self.state)["recorded_stage"], "verified_stage")

    def test_rebound_final_name_after_the_real_move_cannot_complete_the_receipt(self):
        self.qualified(); operation = self.prepare()
        target = self.out/"Chosen"; inode = (self.state/staging.LEAF).stat().st_ino
        native = staging.check_mount_name
        checks = []

        def named(parent, leaf, identity, mount):
            if leaf == target.name and os.path.samestat(os.fstat(parent), self.out.stat()):
                self.assertEqual(target.stat().st_ino, inode)
                with self.another_named_mount(target) as checked:
                    try: return native(parent, leaf, identity, mount)
                    finally: checks.extend(checked)
            return native(parent, leaf, identity, mount)

        with patch.object(staging, "check_mount_name", side_effect=named), \
             self.assertRaisesRegex(Rejected, "original mount"):
            opening.finish(self.state, operation)
        self.assertEqual(len(checks), 1)
        self.assertEqual(target.stat().st_ino, inode); self.assertFalse((self.state/staging.LEAF).exists())
        self.assertFalse((self.state/"worktree.sqlite").exists())
        self.assertEqual(opening.status(self.state)["recorded_stage"], "verified_stage")
        with patch.object(opening, "rename_new_at", side_effect=AssertionError("must acknowledge the same move")):
            self.assertEqual(opening.finish(self.state, operation), (target, self.marker))
        self.assertEqual(target.stat().st_ino, inode)

    def test_unqualified_mount_or_nonexclusive_state_parent_never_captures_or_falls_back(self):
        for gate in ("mode", "mount"):
            with self.subTest(gate=gate):
                self.private.chmod(0o755 if gate == "mode" else 0o700)
                with patch.object(opening, "named_document", side_effect=AssertionError("no source capture")), \
                     patch.object(opening.tempfile, "mkdtemp", side_effect=AssertionError("no public stage")), \
                     patch.object(staging, "same_mount", side_effect=Rejected("separate mount")):
                    with self.assertRaises(Rejected): self.begin()
                self.assertFalse(self.state.exists())
                self.assertEqual(list(self.private.iterdir()), []); self.assertEqual(list(self.out.iterdir()), [])

    def test_visible_state_without_its_own_outer_barrier_cannot_publish_a_worktree(self):
        self.qualified()
        original_sync = os.fsync
        def deny_outer(fd):
            if self.state.is_dir() and os.path.samestat(os.fstat(fd), self.private.stat()):
                raise OSError("STATE name not durable")
            return original_sync(fd)
        with patch.object(opening.os, "fsync", side_effect=deny_outer):
            with self.assertRaisesRegex(OSError, "STATE name not durable"): self.begin()
            op = opening.status(self.state)["operation"]
            source = self.state / staging.LEAF; inode = source.stat().st_ino
            with patch.object(opening, "rename_new_at", side_effect=AssertionError("no public rename before STATE sync")):
                with self.assertRaisesRegex(OSError, "STATE name not durable"): opening.finish(self.state, op)
            self.assertEqual(source.stat().st_ino, inode); self.assertEqual(list(self.out.iterdir()), [])
        with patch.object(staging, "materialize", side_effect=AssertionError("no recapture")):
            dest, marker = opening.finish(self.state, op)
        self.assertEqual((dest.stat().st_ino, marker), (inode, self.marker))

    def test_original_state_under_a_substituted_outer_name_does_not_authorize_publication(self):
        self.qualified(); op = self.prepare()
        old = self.root / "original-private"; self.private.rename(old); self.private.mkdir(mode=0o700)
        (old / self.state.name).rename(self.state)  # The STATE object itself really is the original.
        with patch.object(opening, "rename_new_at", side_effect=AssertionError("no public move")):
            with self.assertRaisesRegex(Rejected, "STATE parent was replaced"): opening.finish(self.state, op)
        self.assertEqual(list(self.out.iterdir()), []); self.assertTrue((self.state / staging.LEAF).is_dir())

    def test_link_as_the_selected_output_parent_is_not_resolved_into_a_different_spelling(self):
        alias = self.root / "selected-link"; alias.symlink_to(self.out, target_is_directory=True)
        with patch.object(opening, "named_document", side_effect=AssertionError("no original capture")):
            with self.assertRaisesRegex(Rejected, "checkout parent"):
                opening.begin(self.archive, alias, "Chosen", self.store, self.state, automatic=False, private_stage=True)
        self.assertEqual(list(self.private.iterdir()), []); self.assertEqual(list(self.out.iterdir()), [])

    def test_permission_loss_at_the_private_seed_barrier_cannot_publish_state(self):
        self.qualified(); real_sync = os.fsync
        for role in ("parent", "seed"):
            with self.subTest(role=role):
                changed = []
                def lose_exclusivity(fd):
                    real_sync(fd)
                    for seed in self.private.iterdir():
                        if ((seed / "opening.sqlite").is_file() and os.path.samestat(os.fstat(fd), seed.stat())):
                            (self.private if role == "parent" else seed).chmod(0o755); changed.append(seed)
                with patch.object(opening.os, "fsync", side_effect=lose_exclusivity):
                    with self.assertRaisesRegex(Rejected, "exclusive to its owner"): self.begin()
                self.private.chmod(0o700)
                self.assertEqual(len(changed), 1); self.assertTrue((changed[0] / staging.LEAF).is_dir())
                self.assertFalse(self.state.exists()); self.assertEqual(list(self.out.iterdir()), [])

    def test_resume_rechecks_actual_store_ancestry_and_a_distinct_same_device_mount(self):
        self.qualified()
        route = self.root / "selected"; route.symlink_to(self.root, target_is_directory=True)
        self.store = route / "store"; op = self.prepare()
        (self.private / "store").mkdir(mode=0o700)
        route.unlink(); route.symlink_to(self.private, target_is_directory=True)
        with patch.object(opening, "rename_new_at", side_effect=AssertionError("no public rename")):
            with self.assertRaisesRegex(Rejected, "contains another"): opening.finish(self.state, op)
            route.unlink(); route.symlink_to(self.root, target_is_directory=True)
            original_mount = staging.mount_id
            def ambiguous(fd):
                if os.path.samestat(os.fstat(fd), self.store.stat()): return b"distinct-bind"
                return original_mount(fd)
            with patch.object(staging, "mount_id", side_effect=ambiguous):
                with self.assertRaisesRegex(Rejected, "ambiguous archive-store mount"): opening.finish(self.state, op)
        self.assertEqual(list(self.out.iterdir()), []); self.assertTrue((self.state / staging.LEAF).is_dir())

    def test_existing_unregistered_stage_is_not_claimed_or_cleaned(self):
        self.qualified()
        make = staging.new_child
        @contextmanager
        def obstruction(area, name, mount):
            if name == staging.LEAF:
                hostile = area.path / name; hostile.mkdir(mode=0o700); (hostile/"keep").write_bytes(b"not ours")
            with make(area, name, mount) as selected: yield selected
        with patch.object(staging, "new_child", side_effect=obstruction):
            with self.assertRaises(FileExistsError): self.begin()
        seed, = self.private.iterdir()
        self.assertEqual((seed / staging.LEAF / "keep").read_bytes(), b"not ours")
        self.assertFalse((seed / "opening.sqlite").exists()); self.assertEqual(list(self.out.iterdir()), [])

    def test_replaced_intermediate_cannot_receive_even_one_member(self):
        self.qualified()
        put = staging._Output.file; foreign = self.root / "foreign"; foreign.mkdir(mode=0o700)
        def move_parent(output, path):
            if path == "nested/a":
                original = output.stage.path / "nested"
                original.rename(output.stage.path / "old-nested")
                original.symlink_to(foreign, target_is_directory=True)
            return put(output, path)
        with patch.object(staging._Output, "file", new=move_parent):
            with self.assertRaises((OSError, Rejected)): self.begin()
        self.assertEqual(list(foreign.iterdir()), []); self.assertFalse(self.state.exists())
        seed, = self.private.iterdir()
        self.assertTrue((seed / staging.LEAF / "nested").is_symlink())
        self.assertFalse((seed / "opening.sqlite").exists()); self.assertEqual(list(self.out.iterdir()), [])

    def test_damaged_private_origin_cannot_start_an_old_public_stage(self):
        self.qualified()
        op = self.prepare()
        with closing(sqlite3.connect(self.state / "opening.sqlite")) as db:
            db.execute("UPDATE opening SET stage=NULL")
            db.commit()
        with patch.object(opening.tempfile, "mkdtemp", side_effect=AssertionError("no fallback")):
            with self.assertRaisesRegex(Rejected, "private checkout decision"): opening.finish(self.state, op)
        self.assertTrue((self.state / staging.LEAF).is_dir()); self.assertEqual(list(self.out.iterdir()), [])
