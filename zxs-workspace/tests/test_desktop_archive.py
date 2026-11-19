from dataclasses import replace
import hashlib
from importlib.util import find_spec
import os
from pathlib import Path
import tempfile
import sys
from unittest.mock import patch
import unittest
import zipfile

from zipworkspace.desktop.selection import Selection, StoreSelection
from zipworkspace.model import Commit, Manifest, Rejected


def write_snapshot(path, *, marker=None, year=2000):
    rows = Manifest()
    content = b"desktop selection"
    rows.add("design/notes", False, len(content), hashlib.sha256(content).hexdigest())
    rows.add("empty", True, 0, None)
    marker = marker or Commit("1"*32, "2"*32, 1, "3"*32, None, rows.digest())
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_STORED) as out:
        for name, data in (("design/notes", content), ("empty/", b"")):
            entry = zipfile.ZipInfo(name, (year, 1, 1, 0, 0, 0))
            entry.external_attr = ((0o40755 if name.endswith("/") else 0o100644) << 16)
            out.writestr(entry, data)
        out.comment = marker.encode()
    return marker


class SelectedOriginal(unittest.TestCase):
    def test_loose_document_can_be_read_but_is_not_an_unpublished_branch_head(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            store = root / "store"; store.mkdir()
            loose = root / "design.zxs"
            first = write_snapshot(loose)
            selection = Selection.open(loose)
            self.assertEqual(selection.browser.list_directory("empty").entries, ())
            chosen_store = StoreSelection.open(store)
            catalog = selection.catalog(chosen_store)
            self.assertEqual(catalog.branches(), [])
            self.assertIsNone(selection.exact)
            self.assertEqual(catalog.select_node(first.operation), catalog.anchor)
            self.assertEqual(catalog.anchor.marker, selection.browser.commit)
            later = replace(first, generation=2, operation="4"*32, parent=first.operation)
            write_snapshot(store / later.filename, marker=later)
            partial = selection.catalog(chosen_store)
            self.assertEqual(partial.branches()[0]["missing_parent"], first.operation)
            with self.assertRaises(Rejected): partial.select_branch(first.branch)
            (store / first.filename).write_bytes(loose.read_bytes())
            complete = selection.catalog(chosen_store)
            head = complete.select_branch(first.branch)
            self.assertEqual(Selection.open(head.path, expected=head).browser.commit, later)

    def test_first_page_and_navigation_never_compute_the_raw_zip_digest(self):
        from zipworkspace import archive, refs
        from zipworkspace.desktop import selection as view
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); path = root/"chosen.zxs"
            marker = write_snapshot(path)
            with patch.object(view,"archive_sha",side_effect=AssertionError("no full raw pass on initial browse")), \
                    patch.object(refs,"archive_sha",side_effect=AssertionError("catalog cannot be implicit")), \
                    patch.object(archive,"_read_checked",wraps=archive._read_checked) as verify:
                selected = Selection.open(path)
                self.assertEqual(verify.call_count,1)
                self.assertIsNone(selected.exact)
                self.assertEqual(selected.browser.list_directory("design").entries[0].sha256,
                                 hashlib.sha256(b"desktop selection").hexdigest())
                self.assertEqual(selected.browser.list_directory("empty").entries,())
                self.assertEqual(verify.call_count,1)
            with patch.object(refs,"archive_sha",wraps=refs.archive_sha) as whole:
                node = selected.catalog(StoreSelection.open(root)).anchor
                self.assertEqual(whole.call_count,1)
            self.assertEqual(node.marker,marker)
            with patch.object(view,"archive_sha",wraps=view.archive_sha) as exact:
                self.assertEqual(Selection.open(path,expected=node).exact,node)
                self.assertEqual(exact.call_count,1)
            with self.assertRaises(Rejected):
                Selection.open(path, expected=replace(node, sha256="0" * 64))

    def test_canonical_anchor_is_verified_once_and_held_through_both_inventories(self):
        from zipworkspace import refs
        with tempfile.TemporaryDirectory() as tmp:
            store = Path(tmp)
            source = store / "setup.zxs"; marker = write_snapshot(source)
            slot = store / marker.filename; source.rename(slot)
            later = replace(marker, generation=2, operation="4"*32, parent=marker.operation)
            peer = store / later.filename; write_snapshot(peer, marker=later)
            selected = Selection.open(slot)
            chosen = StoreSelection.open(store)
            read, digest, members = refs.read_managed_document, refs.archive_sha, refs._members
            for _ in range(2):  # A new catalog always re-verifies both original files.
                documents, hashes, inventories = {}, [], []
                def check(doc, *, display_name):
                    self.assertNotIn(display_name, documents)
                    documents[display_name] = doc
                    return read(doc, display_name=display_name)
                def raw(doc):
                    hashes.append(doc)
                    return digest(doc)
                def inventory(*args):
                    inventories.append(not documents[marker.filename].closed)
                    return members(*args)
                with patch.object(refs, "read_managed_document", side_effect=check), \
                     patch.object(refs, "archive_sha", side_effect=raw), \
                     patch.object(refs, "_members", side_effect=inventory):
                    cat = selected.catalog(chosen)
                self.assertEqual(set(documents), {marker.filename, later.filename})
                self.assertCountEqual(hashes, documents.values())
                self.assertEqual(inventories, [True, True])
                self.assertTrue(all(doc.closed for doc in documents.values()))
                self.assertEqual(cat.select_node(marker.operation), cat.anchor)
                self.assertEqual(cat.select_branch(marker.branch).marker, later)

    def test_same_inode_at_a_different_name_is_not_reused_as_the_store_slot(self):
        from zipworkspace import refs
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); store = root / "store"; store.mkdir()
            loose = root / "loose.zxs"; marker = write_snapshot(loose)
            slot = store / marker.filename
            try: os.link(loose, slot)
            except (OSError, NotImplementedError) as e: self.skipTest(f"hard links unavailable: {e}")
            with patch.object(refs, "archive_sha", wraps=refs.archive_sha) as raw:
                cat = refs.Catalog(loose, store)
            self.assertEqual(raw.call_count, 2)
            self.assertEqual(cat.anchor.path, loose)
            self.assertEqual(cat.select_node(marker.operation).path, slot)
            self.assertEqual(cat.select_branch(marker.branch).marker, marker)

    def test_corrupt_member_outside_root_view_is_rejected_without_a_raw_zip_pass(self):
        from zipworkspace.desktop import selection as view
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)/"corrupt.zxs"
            write_snapshot(path)
            data = path.read_bytes()
            self.assertEqual(data.count(b"desktop selection"),1)
            path.write_bytes(data.replace(b"desktop selection",b"desktop mutation!"))
            with patch.object(view,"archive_sha",side_effect=AssertionError("no raw-digest rescue")):
                with self.assertRaises((Rejected,zipfile.BadZipFile)): Selection.open(path)

    def test_selected_store_and_physical_slot_cannot_be_silently_substituted(self):
        from zipworkspace import refs
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            loose = root / "chosen.zxs"
            marker = write_snapshot(loose)
            store = root / "store"; store.mkdir()
            chosen = StoreSelection.open(store)
            member = store / marker.filename
            member.write_bytes(loose.read_bytes())
            selection = Selection.open(loose)
            self.assertEqual(selection.catalog(chosen).select_branch(marker.branch).marker, marker)
            # Replace only the store slot after the real first inventory; the
            # loose anchor stays valid but cannot authorize the equal-byte replacement.
            members = refs._members
            original = root / "old-slot"
            def replace_after_inventory(*args):
                before = members(*args)
                if not original.exists():
                    member.rename(original)
                    member.write_bytes(original.read_bytes())
                return before
            with patch.object(refs, "_members", side_effect=replace_after_inventory):
                with self.assertRaises(Rejected): selection.catalog(chosen)
            store.rename(root / "old-store")
            store.mkdir()
            (store / marker.filename).write_bytes(loose.read_bytes())
            with self.assertRaises(Rejected): selection.catalog(chosen)
            # A deliberate new picker selection is distinct and can be admitted.
            self.assertEqual(selection.catalog(StoreSelection.open(store)).anchor.marker, marker)

    def test_previously_listed_row_refuses_a_byte_identical_replacement_at_the_same_slot(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); store = root / "store"; store.mkdir()
            loose = root / "loose.zxs"
            marker = write_snapshot(loose)
            slot = store / marker.filename
            slot.write_bytes(loose.read_bytes())
            chosen = StoreSelection.open(store)
            view = Selection.open(loose)
            listed = view.catalog(chosen).select_branch(marker.branch)
            self.assertEqual(Selection.open(slot, expected=listed).exact, listed)
            retained = root / "original-slot"
            slot.rename(retained)  # Keep the predecessor alive: its inode cannot be reused.
            slot.write_bytes(retained.read_bytes())
            # Same storage name, commit and raw ZIP SHA, but not the listed file.
            fresh = view.catalog(chosen).select_branch(marker.branch)
            self.assertEqual((fresh.path, fresh.marker, fresh.sha256), (listed.path, listed.marker, listed.sha256))
            self.assertNotEqual(fresh.observed[:2], listed.observed[:2])
            with self.assertRaises(Rejected): Selection.open(slot, expected=listed)
            self.assertIsNone(Selection.open(slot).exact)
            self.assertEqual(Selection.open(slot, expected=fresh).exact, fresh)

    def test_same_logical_tree_reencoded_is_not_the_selected_original_for_a_write(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = root / "original.zxs"
            marker = write_snapshot(path)
            original = Selection.open(path)
            store = StoreSelection.open(root)
            exact = original.catalog(store).anchor  # The stronger decision binds the raw ZIP bytes.
            replacement = root / "new.zxs"
            write_snapshot(replacement, marker=marker, year=2001)
            replacement.replace(path)
            current = Selection.open(path)
            self.assertEqual(current.browser.commit, marker)
            self.assertIsNone(current.exact)
            self.assertNotEqual(current.catalog(store).anchor.sha256, exact.sha256)
            with self.assertRaises(Rejected): Selection.open(path, expected=exact)
            with self.assertRaises(Rejected): original.catalog(store)


@unittest.skipUnless(find_spec("PySide6"), "optional desktop extra not installed")
class DesktopArchiveWidgets(unittest.TestCase):
    def test_real_window_reuses_the_selected_file_and_shows_the_verified_folders(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); path = root / "chosen.zxs"
            marker = write_snapshot(path)
            win = Window()
            try:
                pane = win.archive
                selected = Selection.open(path)
                pane.show_selection(selected)
                pane.files.selectRow(0); pane.enter()
                self.assertEqual(pane.directory, "design")
                self.assertEqual(pane.files.rows.rows[0][0][0], "notes")
                pane.go_up()
                self.assertEqual(pane.directory, "")
                store = StoreSelection.open(root)
                pane.use_store(store)
                pane.show_catalog(selected.catalog(store))
                self.assertEqual(pane.history.rows.rows[0][1].marker, marker)
                self.assertEqual(pane.branches.rows.rows, ())
            finally:
                win.close()

@unittest.skipUnless(os.name == "posix" and sys.platform.startswith("linux"),
                     "operator-controlled private staging is Linux-only")
class OriginalDesktopCheckout(unittest.TestCase):
    """The positive path uses an admitted real temporary mount, never a fake ext4 success."""
    def setUp(self):
        from zipworkspace import private_stage
        temp = tempfile.TemporaryDirectory(); self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.private, self.parent, self.store = (self.root / n for n in ("states", "work", "archives"))
        for path in (self.private, self.parent, self.store): path.mkdir(mode=0o700)
        a = os.open(self.private, os.O_DIRECTORY); b = os.open(self.parent, os.O_DIRECTORY)
        try:
            try: private_stage.same_mount(a, b)
            except (OSError, Rejected) as exc: self.skipTest("requires a genuine admitted mount: " + str(exc))
        finally: os.close(b); os.close(a)
        self.loose = self.root / "arbitrary-name.zxs"
        self.marker = write_snapshot(self.loose)
        self.target, self.state = self.parent / "Design", self.private / "Design"

    def choice(self, snapshot=None, *, review=True):
        from zipworkspace.desktop.checkout_action import CheckoutChoice
        plan = CheckoutChoice(snapshot or Selection.open(self.loose))
        self.addCleanup(plan.close)
        plan.pick_store(self.store); plan.pick_target(self.parent); plan.pick_private(self.private)
        if review:
            msg = plan.review("Design")
            for item in (self.target, self.state, self.store, self.marker.operation, plan.operation):
                self.assertIn(str(item), msg)
        return plan

    def test_loose_selected_original_creates_an_actual_cli_checkout_without_reopening_source(self):
        from zipworkspace import opening, worktree
        plan = self.choice()
        self.assertFalse(self.target.exists()); self.assertFalse(any(self.private.iterdir()))
        with patch.object(opening, "named_document", side_effect=AssertionError("must borrow original file")):
            self.assertEqual(plan.run(), (self.target, self.marker))
        self.assertEqual((self.target / "design/notes").read_bytes(), b"desktop selection")
        self.assertTrue((self.target / "empty").is_dir())
        self.assertEqual((self.state / "original.zxs").read_bytes(), self.loose.read_bytes())
        self.assertEqual(worktree.status(self.state)["base_operation"], self.marker.operation)
        self.assertEqual(opening.status(self.state)["operation"], plan.operation)
        self.assertFalse(any(self.store.iterdir()))
        with self.assertRaises(Rejected): plan.run()

    def test_head_advancing_after_selection_does_not_change_which_historical_node_is_written(self):
        from zipworkspace import opening
        original = self.store / self.marker.filename
        self.loose.rename(original)
        current = Selection.open(original)
        exact = current.catalog(StoreSelection.open(self.store)).select_branch(self.marker.branch)
        plan = self.choice(Selection.open(exact.path, expected=exact))
        newer = replace(self.marker, generation=2, operation="4"*32, parent=self.marker.operation)
        write_snapshot(self.store / newer.filename, marker=newer)
        with patch.object(Selection, "catalog", side_effect=AssertionError("not another head selection")):
            self.assertEqual(plan.run(), (self.target, self.marker))
        self.assertEqual(opening.status(self.state)["source_node"], exact.marker.operation)
        self.assertEqual(hashlib.sha256((self.state/"original.zxs").read_bytes()).hexdigest(), exact.sha256)

    def test_path_replacement_after_review_cannot_redirect_any_of_the_four_selections(self):
        from zipworkspace import opening
        for original in (self.loose, self.store, self.parent, self.private):
            with self.subTest(original=original):
                plan = self.choice()
                backup = self.root / "held-original"
                original.rename(backup)
                if backup.is_dir(): original.mkdir(mode=0o700)
                else: original.write_bytes(backup.read_bytes())  # Equal bytes, distinct original.
                try:
                    with patch.object(opening, "_source", side_effect=AssertionError("no source capture")):
                        with self.assertRaisesRegex(Rejected, "尚未安装原登记"): plan.run()
                    self.assertFalse(any(self.private.iterdir())); self.assertFalse(any(self.parent.iterdir()))
                finally:
                    if original.is_dir(): original.rmdir()
                    else: original.unlink()
                    backup.rename(original)

    def test_new_collision_and_later_mount_change_are_refused_before_any_preparation(self):
        from zipworkspace import opening, private_stage
        plan = self.choice()
        self.target.mkdir(); (self.target / "mine").write_bytes(b"untouched")
        with self.assertRaisesRegex(Rejected, "占用"): plan.run()
        self.assertEqual((self.target / "mine").read_bytes(), b"untouched")
        other = self.choice(review=False)
        native_id = private_stage.mount_id
        def distinct(fd):
            return b"different-actual-mount" if os.path.samestat(os.fstat(fd), self.parent.stat()) else native_id(fd)
        with patch.object(private_stage, "mount_id", side_effect=distinct), \
             patch.object(opening, "_source", side_effect=AssertionError("no ZIP copy or private preparation")):
            with self.assertRaisesRegex(Rejected, "same original mount"): other.review("Other")
        self.assertFalse(any(self.private.iterdir()))

    def test_failed_materialization_is_not_a_durable_receipt_or_resume_instruction(self):
        from zipworkspace import private_stage
        plan = self.choice()
        with patch.object(private_stage, "materialize", side_effect=OSError("stop during private preparation")):
            with self.assertRaises(Rejected) as fail: plan.run()
        self.assertIn("没有可续作的回执", str(fail.exception))
        self.assertNotIn("resume-checkout", str(fail.exception))
        self.assertFalse(self.state.exists()); self.assertFalse(self.target.exists())
        seed, = self.private.iterdir()
        self.assertEqual((seed / "original.zxs").read_bytes(), self.loose.read_bytes())

    def test_lost_reply_after_real_cross_parent_move_names_only_original_durable_operation(self):
        from zipworkspace import opening, private_stage
        plan = self.choice(); native = opening.rename_new_at
        def lose_reply(fd, src, dst, **kwargs):
            native(fd, src, dst, **kwargs)
            if src == private_stage.LEAF and kwargs.get("target_fd") is not None:
                raise InterruptedError("lost move response")
        with patch.object(opening, "rename_new_at", side_effect=lose_reply):
            with self.assertRaises(Rejected) as fail: plan.run()
        self.assertIn("已读到本次原登记", str(fail.exception)); self.assertIn(plan.operation, str(fail.exception))
        self.assertIn("resume-checkout", str(fail.exception))
        self.assertEqual(opening.status(self.state)["recorded_stage"], "verified_stage")
        inode = self.target.stat().st_ino
        self.assertEqual(opening.finish(self.state, plan.operation), (self.target, self.marker))
        self.assertEqual(self.target.stat().st_ino, inode)
        self.assertEqual(opening.status(self.state)["recorded_stage"], "attached")

    def test_substituted_state_after_initial_install_never_opens_or_resumes_the_replacement(self):
        from zipworkspace import opening
        plan = self.choice(); native = opening.finish
        saved = self.private / "retained-original"
        def exchange(state, operation, **kwargs):
            state.rename(saved); state.mkdir(mode=0o700)
            (state / "opening.sqlite").write_bytes(b"not the original database")
            with patch.object(opening.sqlite3, "connect", side_effect=AssertionError("no replacement SQLite open")):
                return native(state, operation, **kwargs)
        with patch.object(opening, "finish", side_effect=exchange):
            with self.assertRaises(Rejected) as fail: plan.run()
        self.assertIn("签出结果未知", str(fail.exception)); self.assertNotIn("resume-checkout", str(fail.exception))
        self.assertEqual((self.state / "opening.sqlite").read_bytes(), b"not the original database")
        self.assertTrue((saved / "prepared-worktree/design/notes").is_file())
        self.assertFalse(self.target.exists())

    @unittest.skipUnless(find_spec("PySide6"), "optional desktop extra not installed")
    def test_widget_cancellation_stale_confirmation_and_reentry_do_not_create_a_second_operation(self):
        from concurrent.futures import Future
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop import checkout_pane as ui
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        win = Window()
        self.addCleanup(win.close)
        pane = win.archive.checkout
        def dispatch(title, work, apply, *, failed=None):
            future = Future()
            try: result = work()
            except Exception as error:
                future.set_exception(error); failed(error)
            else:
                future.set_result(result); apply(result)
            return future
        for mode in ("cancel", "selection_changed", "accept"):
            with self.subTest(mode=mode):
                win.archive.show_selection(Selection.open(self.loose))
                def decide(_parent, _title, detail, *, attestation):
                    self.assertIn(str(self.target), detail); self.assertIn("独占", attestation)
                    pane.pick()  # The confirmation is already active; must not open another picker.
                    if mode == "selection_changed": pane.set_snapshot(None)
                    return mode != "cancel"
                with patch.object(win, "job", side_effect=dispatch), \
                     patch.object(ui.QFileDialog, "getExistingDirectory",
                                  side_effect=list(map(str, (self.store,self.parent,self.private)))) as picks, \
                     patch.object(ui.QInputDialog, "getText", return_value=("Design", True)), \
                     patch.object(ui, "confirm", side_effect=decide):
                    pane.pick()
                self.assertEqual(picks.call_count, 3)
                self.assertFalse(pane.active)
                self.assertEqual(self.target.is_dir(), mode == "accept")
                self.assertEqual(self.state.is_dir(), mode == "accept")
        self.assertIn(self.marker.operation, pane.result.text())
        self.assertEqual((self.target/"design/notes").read_bytes(), b"desktop selection")
