"""Source fixtures for held desktop browsing and original-member literal preview."""
from concurrent.futures import Future
import hashlib
from importlib.util import find_spec
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

from zipworkspace.desktop import text_preview
from zipworkspace.desktop.selection import Selection
from zipworkspace.model import Commit, Manifest, Rejected


class DesktopTextPreview(unittest.TestCase):
    def setUp(self):
        room = tempfile.TemporaryDirectory()
        self.addCleanup(room.cleanup)
        self.root = Path(room.name)
        self.count = 0

    def archive(self, payload, *, name="folder/notes"):
        self.count += 1
        path = self.root / f"copy-{self.count}.zxs"
        rows = Manifest()
        with path.open("xb") as raw, zipfile.ZipFile(raw, "w") as z:
            for leaf, data in ((name, payload), ("other/notes", b"unselected")):
                rows.add(leaf, False, len(data), hashlib.sha256(data).hexdigest())
                z.writestr(leaf, data)
            z.comment = Commit("1"*32, "2"*32, 1, "3"*32, None, rows.digest()).encode()
        selected = Selection.open(path, retain=True)
        self.addCleanup(selected.release_display)
        return selected

    def get(self, selected, *, check=lambda: None, indexed=False):
        return text_preview.prepare(selected, "folder", selected.browser.entry("folder", "notes"),
                                    check=check, indexed=indexed)

    def test_only_original_selected_body_and_plain_unicode_no_named_reopen(self):
        raw = b"<literal>\r\n" + "中文𠀀".encode()
        chosen = self.archive(raw)
        fd = chosen.held.fileno()
        os.lseek(fd, 19, os.SEEK_SET)
        with patch.object(os, "open", side_effect=AssertionError("preview never opens the source by name")):
            report = self.get(chosen)
        self.assertEqual(report.path, "folder/notes")
        self.assertEqual(report.bytes_total, len(raw))
        self.assertEqual(report.bytes_shown, len(raw))
        self.assertEqual(report.pages, ("<literal>\n中文𠀀",))
        self.assertEqual(os.lseek(fd, 0, os.SEEK_CUR), 19)
        self.assertEqual(self.get(self.archive(b"")).pages, ("",))
        wrong = chosen.browser.entry("other", "notes")
        with self.assertRaises(Rejected):
            text_preview.prepare(chosen, "folder", wrong, check=lambda: None)

    def test_preview_never_silently_replaces_bad_or_control_utf8(self):
        for sample in (b"a\xff", b"a\r", "a\u202eb".encode(), "a\ufe0f".encode()):
            with self.subTest(sample=sample):
                with self.assertRaises(Rejected): self.get(self.archive(sample))

    def test_large_prefix_proves_complete_member_and_never_cuts_a_scalar_or_crlf(self):
        pre = b"a" * (text_preview.LIMIT - 1)
        for tail in ("𠀀tail".encode(), b"\r\nnot necessarily UTF-8\xff"):
            with self.subTest(tail=tail):
                raw = pre + tail
                report = self.get(self.archive(raw))
                self.assertEqual((report.bytes_total, report.bytes_shown), (len(raw), len(pre)))
                self.assertEqual("".join(report.pages).encode(), pre)
        for tail in (b"\xf0\x80bad", "\u202eafter".encode(), b"\rx"):
            with self.assertRaises(Rejected): self.get(self.archive(pre + tail))

    def test_indexed_read_reuses_original_body_offset_and_only_the_two_verified_seam_blocks(self):
        block = text_preview.CHUNK
        payload = b"a"*(block-2) + "𠀀".encode() + b"b"*(block-3) + b"\r\nliteral end"
        chosen = self.archive(payload)
        fd = chosen.held.fileno()
        os.lseek(fd, 19, os.SEEK_SET)
        proof = self.get(chosen, indexed=True)
        self.assertEqual(proof.bytes_total, len(payload))
        self.assertEqual(proof.first_page, "a" * text_preview.UNITS_PER_PAGE)
        self.assertEqual(len(proof.blocks), 3 * text_preview._BLOCK.size)
        self.assertEqual((proof._block(0)[1], proof._block(1)[1], proof._block(2)[1]), (0, 2, 1))

        original_read = text_preview._Checked.read
        reads = []
        def observed_read(f, size):
            reads.append((f.tell(), size))
            return original_read(f, size)

        with patch.object(text_preview._Checked, "read", observed_read), \
             patch.object(text_preview, "stored_member", side_effect=AssertionError("no indexed ZIP reread")), \
             patch.object(os, "open", side_effect=AssertionError("no by-name archive opening")):
            value = proof.page(proof.count - 1, check=lambda: None)
        self.assertEqual(value, "\nliteral end")
        self.assertEqual(reads, [(proof.body_at + block, block),
                                (proof.body_at + 2*block, len(payload) - 2*block)])
        self.assertEqual(os.lseek(fd, 0, os.SEEK_CUR), 19)
        self.assertTrue(proof.page(proof._block(1)[0], check=lambda: None).startswith("𠀀b"))
        with self.assertRaises(Rejected):
            proof.page(proof.count, check=lambda: None)

        def cancelled():
            raise Rejected("cancel before any page")
        with self.assertRaisesRegex(Rejected, "cancel before"):
            proof.page(0, check=cancelled)
        chosen.release_display()
        with self.assertRaises(Rejected):
            proof.page(0, check=lambda: None)

    def test_large_index_never_shows_invalid_text_or_a_mismatched_block(self):
        for tail in (b"\xff", "\u202e".encode(), b"\rtext"):
            with self.subTest(tail=tail):
                chosen = self.archive(b"a"*(text_preview.CHUNK-1) + tail)
                self.assertEqual(self.get(chosen).bytes_shown, text_preview.LIMIT)
                with self.assertRaisesRegex(Rejected, "全文含无效"):
                    self.get(chosen, indexed=True)

        chosen = self.archive(b"v"*text_preview.CHUNK + b"correct tail")
        proof = self.get(chosen, indexed=True)
        original_read = text_preview._Checked.read
        for short in (False, True):
            def wrong(f, size):
                actual = original_read(f, size)
                return actual[:-1] if short else b"Z" + actual[1:]
            with self.subTest(short=short), patch.object(text_preview._Checked, "read", wrong):
                with self.assertRaisesRegex(Rejected, "文本块与原核验字节不同"):
                    proof.page(proof.count - 1, check=lambda: None)

    def test_page_geometry_is_bounded_with_an_unchanged_join_and_cancellable_scan(self):
        text = ("a"*8191) + "𠀀" + "\n"*800 + "end"
        report = self.get(self.archive(text.encode()))
        self.assertEqual("".join(report.pages), text)
        for page in report.pages:
            self.assertLessEqual(len(page.encode("utf-16-le"))//2, text_preview.UNITS_PER_PAGE)
            self.assertLessEqual(page.count("\n"), text_preview.LINES_PER_PAGE)
        stop = 0
        def check():
            nonlocal stop
            stop += 1
            if stop == 4: raise Rejected("fixture cancellation")
        with self.assertRaisesRegex(Rejected, "fixture cancellation"):
            self.get(self.archive(text.encode()), check=check)

    @unittest.skipUnless(os.environ.get("ZIP_WORKSPACE_GUI_TESTS") == "1" and find_spec("PySide6"),
                         "optional Qt source fixture")
    def test_qt_reads_only_after_confirmation_and_keeps_old_result_out_of_a_new_row(self):
        from PySide6.QtWidgets import QApplication, QPlainTextEdit
        from zipworkspace.desktop import archive_pane
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        win = Window()
        self.addCleanup(win.close)
        pane = win.archive
        chosen = self.archive(b"<literal>")
        pane.show_selection(chosen)

        def select_original():
            pane.show_folder("folder")
            pane.files.selectRow(0)
            self.assertTrue(pane.preview_file.isEnabled())

        select_original()
        with patch.object(archive_pane, "confirm", return_value=False), \
             patch.object(win.pool, "submit", side_effect=AssertionError("no reading without confirmation")):
            pane.preview_file.click()

        def pending():
            select_original()
            task = Future()
            with patch.object(archive_pane, "confirm", return_value=True), \
                 patch.object(win.pool, "submit", return_value=task) as worker:
                pane.preview_file.click()
            self.assertFalse(win.transfer_cancel.isHidden())
            return task, worker.call_args.args[0]

        first, work = pending()
        first.set_result(work())
        pane.show_folder("other"); pane.files.selectRow(0)  # same basename, other original row
        win.poll()
        self.assertIsNone(pane.text_dialog)

        second, work = pending()
        second.set_result(work())
        win.poll()
        self.assertIsNotNone(pane.text_dialog)
        literal = pane.text_dialog.findChild(QPlainTextEdit)
        self.assertTrue(literal.isReadOnly())
        self.assertEqual(literal.toPlainText(), "<literal>")
        pane.close_text()
        self.assertIsNone(pane.text_dialog)

    @unittest.skipUnless(os.environ.get("ZIP_WORKSPACE_GUI_TESTS") == "1" and find_spec("PySide6"),
                         "optional Qt source fixture")
    def test_new_window_does_not_inherit_a_selection_and_close_snapshot_only_releases_browsing(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop.window import DesktopWindows
        app = QApplication.instance() or QApplication([])
        manager = DesktopWindows()
        left = manager.open()
        self.addCleanup(left.close)
        chosen = self.archive(b"keep other window")
        left.archive.show_selection(chosen)
        right = manager.open()
        self.addCleanup(right.close)
        self.assertIsNot(left.pool, right.pool)
        self.assertIsNone(right.archive.selection)
        self.assertIsNone(right.archive.store)
        self.assertIsNone(right.archive.checkout.live)
        self.assertIs(left.archive.selection, chosen)
        self.assertFalse(chosen.held.closed)
        with patch.object(left.archive.checkout, "close_session", side_effect=AssertionError("not a window close")):
            left.archive.close_snapshot()
        self.assertIsNone(left.archive.selection)
        self.assertTrue(chosen.held.closed)
        self.assertIsNone(right.archive.selection)

    def test_external_startup_accepts_one_literal_absolute_path_without_file_io(self):
        from zipworkspace.desktop import startup_snapshot

        selected = str(self.root / "with spaces 目录.ZXS")
        with patch.object(os, "stat", side_effect=AssertionError("no path IO before confirmation")):
            self.assertIsNone(startup_snapshot([]))
            self.assertEqual(startup_snapshot([selected]), selected)
            for arguments in (["--platform", selected], ["-style:evil.zxs"], [selected, selected],
                              ["some.zxs"], ["https://example.invalid/a.zxs"], ["file://" + selected],
                              [str(self.root / "file.zip")], [selected + os.sep], [selected + "\x00"]):
                with self.subTest(arguments=arguments), self.assertRaises(ValueError):
                    startup_snapshot(arguments)

    @unittest.skipUnless(os.environ.get("ZIP_WORKSPACE_GUI_TESTS") == "1" and find_spec("PySide6"),
                         "optional Qt source fixture")
    def test_drop_cannot_move_or_read_before_consent_and_opens_only_an_isolated_window(self):
        from PySide6.QtCore import QMimeData, QPointF, QUrl, Qt
        from PySide6.QtGui import QDropEvent
        from PySide6.QtWidgets import QApplication, QDialog, QDialogButtonBox
        from zipworkspace.desktop.window import DesktopWindows

        app = QApplication.instance() or QApplication([])
        manager = DesktopWindows()
        left = manager.open()
        self.addCleanup(left.close)
        original = self.archive(b"original window")
        offered = self.archive(b"opened only by explicit assent")
        offered.release_display()  # Only the real worker may acquire the new browsing handle.
        left.archive.show_selection(original)

        here = QUrl.fromLocalFile(str(offered.path))
        copy, move = Qt.DropAction.CopyAction, Qt.DropAction.MoveAction

        def drop(urls, actions=copy | move):
            data = QMimeData()
            data.setUrls(urls)
            evt = QDropEvent(QPointF(5, 5), actions, data,
                             Qt.MouseButton.LeftButton, Qt.KeyboardModifier.ShiftModifier)
            left.snapshot_drop.dropEvent(evt)
            return evt.isAccepted(), evt.dropAction()

        with patch.object(Selection, "open", side_effect=AssertionError("before readiness and consent")):
            for urls, actions in (([here], move), ([here, here], copy),
                                  ([QUrl("https://example.invalid/a.zxs")], copy),
                                  ([QUrl("file://example.invalid/a.zxs")], copy),
                                  ([QUrl.fromLocalFile(str(self.root / "file.zip"))], copy)):
                self.assertFalse(drop(urls, actions)[0])
            self.assertEqual(len(manager.windows), 1)
            self.assertEqual(drop([here]), (True, copy))
            cancelled = list(manager.windows.values())[-1]
            self.assertIsNone(cancelled.pending)
            self.assertFalse(drop([here])[0])  # don't enqueue repeated unattended offers
            first = cancelled.findChild(QDialog, "open-offered-snapshot")
            first.findChild(QDialogButtonBox).button(QDialogButtonBox.StandardButton.Cancel).click()
            self.assertFalse(cancelled.isVisible())
            self.assertIsNone(manager.request)
            self.assertEqual(drop([here]), (True, copy))
            right = list(manager.windows.values())[-1]
            self.addCleanup(right.close)
            self.assertIsNone(right.archive.selection)
            self.assertIsNone(right.pending)

        self.assertIsNone(right.archive.store)
        self.assertIsNone(right.archive.checkout.live)
        confirm = right.findChild(QDialog, "open-offered-snapshot")
        pending = Future()
        with patch.object(right.pool, "submit", return_value=pending) as submit:
            confirm.findChild(QDialogButtonBox).button(QDialogButtonBox.StandardButton.Open).click()
            submit.assert_called_once()
        pending.set_result(submit.call_args.args[0]())
        right.poll()
        self.assertEqual(right.archive.selection.path, offered.path)
        self.assertFalse(right.archive.selection.held.closed)
        self.assertIs(left.archive.selection, original)
        self.assertFalse(original.held.closed)
        self.assertIsNone(manager.request)

    @unittest.skipUnless(os.environ.get("ZIP_WORKSPACE_GUI_TESTS") == "1" and find_spec("PySide6"),
                         "optional Qt source fixture")
    def test_qt_large_jump_uses_the_explicit_choice_and_a_late_page_cannot_return(self):
        from PySide6.QtWidgets import QApplication, QLineEdit, QPlainTextEdit, QPushButton
        from zipworkspace.desktop import archive_pane
        from zipworkspace.desktop.window import Window

        app = QApplication.instance() or QApplication([])
        win = Window()
        self.addCleanup(win.close)
        pane = win.archive
        selected = self.archive(b"r" * text_preview.CHUNK + b"<verified end>")
        pane.show_selection(selected)
        pane.show_folder("folder")
        pane.files.selectRow(0)
        self.assertTrue(pane.preview_whole.isEnabled())
        first = Future()
        with patch.object(archive_pane, "confirm", return_value=True) as consent, \
             patch.object(win.pool, "submit", return_value=first) as task:
            pane.preview_whole.click()
            self.assertIn(f"{text_preview.CHUNK + len(b'<verified end>'):,}", consent.call_args.args[2])
        result = task.call_args.args[0]()
        first.set_result(result)
        win.poll()
        authorized = pane.text_dialog
        self.assertEqual(authorized.objectName(), "indexed-text-preview")
        view = authorized.findChild(QPlainTextEdit)
        jump = authorized.findChild(QLineEdit, "indexed-text-page-number")
        button = authorized.findChild(QPushButton, "indexed-text-go")
        self.assertEqual(view.toPlainText(), "r" * text_preview.UNITS_PER_PAGE)
        jump.setText(str(result[0].count))

        second = Future()
        with patch.object(win.pool, "submit", return_value=second) as task:
            button.click()
        self.assertEqual(view.toPlainText(), "")
        real_read = task.call_args.args[0]()
        self.assertEqual(real_read, ("<verified end>", None))
        pane.show_folder("other")
        pane.files.selectRow(0)
        self.assertIsNone(pane.text_dialog)
        self.assertEqual(view.toPlainText(), "")
        self.assertEqual(jump.text(), "")
        win.inform("当前在另一个目录")
        second.set_result(real_read)  # delivery can be delayed until the original dialog is closed.
        win.poll()
        self.assertIsNone(pane.text_dialog)
        self.assertEqual(win.note.text(), "当前在另一个目录")

    @unittest.skipUnless(os.name == "posix", "replace a read-open name on POSIX")
    def test_equal_bytes_replacement_is_not_the_held_original(self):
        chosen = self.archive(b"read only")
        old = self.root / "previous.zxs"
        chosen.path.rename(old)
        chosen.path.write_bytes(old.read_bytes())
        with self.assertRaisesRegex(Rejected, "原件已改变"):
            self.get(chosen)
