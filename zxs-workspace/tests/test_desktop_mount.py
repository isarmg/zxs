"""Desktop mount handoff fixtures. No fixture runs a mount helper or calls the kernel to mount."""
from concurrent.futures import Future
import hashlib
from importlib.util import find_spec
import json
import os
from pathlib import Path
import socket
import sys
import tempfile
import threading
import unittest
from unittest import mock
import zipfile

from zipworkspace.desktop.selection import Selection
from zipworkspace.model import Commit, Manifest, Rejected

if sys.platform == "linux":
    from zipworkspace import _mount_child as child, linux_mount
    from zipworkspace.desktop import mount_action as action


@unittest.skipUnless(sys.platform == "linux", "Linux descriptor handoff")
class DesktopMount(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.realpoint, self.spelling = self.root / "view here", "/view here"
        self.realpoint.mkdir(mode=0o700)
        self.archive = self.root / "original.zxs"
        data = b"selected member bytes"
        manifest = Manifest()
        manifest.add("a", False, len(data), hashlib.sha256(data).hexdigest())
        self.marker = Commit("1"*32, "2"*32, 1, "3"*32, None, manifest.digest())
        with zipfile.ZipFile(self.archive, "w") as z:
            z.writestr("a", data)
            z.comment = self.marker.encode()
        self.selection = Selection.open(self.archive, retain=True)
        self.addCleanup(self.selection.release_display)
        actual = os.open
        def redirected(path, flags, *args, **kw):
            # Only the synthetic mountpoint universe is redirected; the chosen ZIP below is real.
            spelling = os.fspath(path)
            if spelling == "/": path = self.root
            elif spelling == self.spelling: path = self.realpoint
            return actual(path, flags, *args, **kw)
        for patch in (mock.patch.object(os, "open", side_effect=redirected),
                      mock.patch.object(linux_mount, "local_mount", return_value=b"fixture"),
                      mock.patch.object(action, "_ordinary_user", side_effect=os.getuid),
                      mock.patch.object(child, "_ordinary_user", side_effect=os.getuid)):
            patch.start(); self.addCleanup(patch.stop)

    def choice(self):
        choice = action.MountChoice(self.selection, self.spelling)
        self.addCleanup(choice.close)
        return choice

    def inherited(self):
        choice = self.choice()
        detail = choice.review()
        self.assertIn(self.marker.operation, detail); self.assertIn(self.spelling, detail)
        with mock.patch.object(action.subprocess, "Popen", side_effect=AssertionError("no confirmation")):
            with self.assertRaises(Rejected): choice.launch()
        def transfer(cmd, **kw):
            self.assertFalse(kw["shell"]); self.assertTrue(kw["close_fds"])
            self.assertTrue(kw["start_new_session"]); self.assertEqual(kw["cwd"], "/")
            self.assertEqual(cmd[:3], [sys.executable, "-I", "-B"])
            self.assertTrue(os.path.isabs(cmd[3]) and os.path.samefile(cmd[3], child.__file__))
            fds = tuple(map(int, cmd[4:]))
            self.assertEqual(fds, kw["pass_fds"])
            self.link = socket.socket(fileno=os.dup(fds[0])); self.addCleanup(self.link.close)
            self.req = json.loads(self.link.recv(child.LIMIT))
            self.assertNotIn(str(self.archive), self.req.values())
            self.doc = os.fdopen(os.dup(fds[1]), "rb"); self.addCleanup(self.doc.close)
            self.assertTrue(os.path.samestat(os.fstat(self.doc.fileno()), os.fstat(self.selection.held.fileno())))
            self.chain = tuple(os.dup(fd) for fd in fds[2:])
            for fd in self.chain: self.addCleanup(os.close, fd)
            self.proc = mock.Mock(); self.proc.poll.return_value = None
            return self.proc
        with mock.patch.object(action.subprocess, "Popen", side_effect=transfer) as start:
            posted = choice.launch(confirmed=True)
            start.assert_called_once()
        self.addCleanup(posted._disconnect)
        self.addCleanup(posted.close_observation)
        return posted

    def test_real_inherited_snapshot_and_ancestors_serve_the_displayed_node_without_opening_its_name(self):
        self.inherited()
        display = self.selection.held.fileno()
        self.req["sha"] = hashlib.sha256(self.archive.read_bytes()).hexdigest()
        os.lseek(display, 13, os.SEEK_SET)
        def driver(view, point, check, *, token, accepted):
            check()
            self.assertEqual(view.read(view.lookup(1,b"a"), 3, 5), b"ected")
            self.assertEqual(point, Path(self.spelling)); self.assertEqual(view.commit, self.marker)
            accepted()
        events = []
        with mock.patch("zipworkspace.document.named_document", side_effect=AssertionError("no second pathname open")), \
             mock.patch.object(child, "observe_mount_receipt", return_value=b"123"):
            child.mount_selected(self.req, self.doc, self.chain, lambda: events.append(b"G"),
                                 events.append, driver=driver)
        self.assertEqual(events, [b"G", b"I", b"M123"])
        self.assertEqual(os.lseek(display, 0, os.SEEK_CUR), 13)

    def test_substituted_equal_bytes_or_different_original_chain_never_enters_driver(self):
        choice = self.choice()
        self.archive.rename(self.root/"saved.zxs")
        self.archive.write_bytes((self.root/"saved.zxs").read_bytes())
        with self.assertRaises(Rejected): choice.review()
        self.assertFalse(self.selection.held.closed)
        self.selection.release_display()
        self.selection = Selection.open(self.archive, retain=True); self.addCleanup(self.selection.release_display)
        self.inherited()
        self.realpoint.rename(self.root/"old-point"); self.realpoint.mkdir(mode=0o700)
        with self.assertRaises(Rejected):
            child.mount_selected(self.req, self.doc, self.chain, mock.Mock(side_effect=AssertionError("no gate")),
                                 mock.Mock(), driver=mock.Mock(side_effect=AssertionError("no driver")))

    def test_historical_raw_mismatch_and_last_name_occupancy_refuse_before_mount(self):
        self.inherited()
        with self.assertRaises(Rejected):
            child.mount_selected(dict(self.req, sha="0"*64), self.doc, self.chain,
                                 mock.Mock(side_effect=AssertionError("no consent")), mock.Mock(),
                                 driver=mock.Mock(side_effect=AssertionError("not mounted")))
        entered = []
        def driver(view, point, check, **kw):
            check(); entered.append("init")
        with self.assertRaises(Rejected):
            child.mount_selected(self.req, self.doc, self.chain,
                                 lambda: (self.realpoint/"occupied").touch(), mock.Mock(), driver=driver)
        self.assertEqual(entered, [])

    def test_inherited_scope_cannot_be_passed_beyond_this_helper_exec(self):
        fds = [os.dup(self.selection.held.fileno()) for _ in range(4)]
        try:
            original = os.fstat(fds[0])
            for fd in fds: os.set_inheritable(fd, True)
            child.seal_handoff(fds)
            for fd in fds:
                self.assertFalse(os.get_inheritable(fd))
                self.assertTrue(os.path.samestat(original, os.fstat(fd)))
        finally:
            for fd in fds: os.close(fd)

    def test_cancel_during_index_and_out_of_order_approval_never_release_the_gate(self):
        p = self.inherited()
        self.assertTrue(p.cancel())
        ready, go = threading.Event(), threading.Event()
        ready.set()
        abandon = mock.Mock()
        child.await_decision(self.link, ready, go, abandon)
        abandon.assert_called_once_with(0); self.assertFalse(go.is_set())
        with self.assertRaises(Rejected): p.grant()
        a, b = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
        with a,b:
            ready.clear(); a.send(b"G")
            child.await_decision(b, ready, go, abandon)
            self.assertFalse(go.is_set())
        self.proc.terminate.assert_not_called(); self.proc.kill.assert_not_called()

    def test_one_consent_then_actual_receipt_exit_is_not_evidence_of_unmount(self):
        p = self.inherited()
        self.assertIn("可取消",p.status())
        self.link.send(b"R"); p.poll(); self.assertTrue(p.ready)
        p.grant(); self.assertEqual(self.link.recv(2), b"G")
        self.assertFalse(p.cancel())
        with self.assertRaises(Rejected): p.grant()
        self.link.send(b"I"); self.link.send(b"A"); p.poll()
        self.assertIn("未核实",p.status()); self.assertIsNone(p.mid)
        self.link.send(b"M73"); p.poll()
        self.assertIn("当时 ID 73",p.status()); self.assertIn("zip-workspace-ro-"+p.token, p.instructions())
        self.assertIn("fusermount3 -u -- '/view here'",p.instructions())
        self.proc.poll.return_value = 0
        p.poll(); self.assertIn("未据此认定已经卸载",p.status())
        self.proc.terminate.assert_not_called(); self.proc.kill.assert_not_called()

    def test_mountinfo_requires_one_exact_escaped_point_old_parent_tag_type_and_read_only_flags(self):
        token, path = "9"*32, Path("/private/view here")
        line = b"73 5 0:8 / /private/view\\040here ro,nosuid,nodev - fuse.zip-workspace zip-workspace-ro-"+token.encode()+b" ro\n"
        self.assertEqual(linux_mount.mount_receipt(line,path,token,b"5"),b"73")
        for wrong in (line+line, line.replace(b"73 5 ",b"73 6 "), line.replace(b"ro,nosuid",b"rw,nosuid"),
                      line.replace(b"fuse.zip-workspace",b"ext4"), line.replace(token.encode(),b"7"*32),
                      line.replace(b"view\\040here",b"view"), line.replace(b"73 5 ",b"5 5 ")):
            self.assertIsNone(linux_mount.mount_receipt(wrong,path,token,b"5"))

    def test_observation_refuses_reused_id_stacked_point_moved_label_or_incomplete_table(self):
        token, path = "9"*32, Path("/private/view here")
        root = b"5 1 8:1 / / rw - ext4 /dev/sda1 rw\n"
        original = (b"73 5 0:8 / /private/view\\040here ro,nosuid,nodev - fuse.zip-workspace "
                    + ("zip-workspace-ro-"+token).encode()+b" ro\n")
        def observe(rows): return action.mount_presence(rows, path, token, b"5", b"73")
        self.assertEqual(observe(root), "absent")
        self.assertEqual(observe(root+original), "present")
        for altered in (original+original,
                        original.replace(b"view\\040here", b"elsewhere"),
                        original.replace(b"zip-workspace-ro-", b"other-"),
                        original.replace(b"73 5 ", b"73 6 "),
                        b"74 5 0:8 / /private/view\\040here rw - tmpfs other rw\n",
                        b"73 5 0:8 / /elsewhere rw - tmpfs other rw\n"):
            self.assertEqual(observe(root+altered), "unknown")
        for invalid in (b"", root+original[:-1], root+b"truncated record\n"):
            with self.assertRaises(Rejected): observe(invalid)

    def test_exit_observation_requires_original_underlying_directory_and_reconfirms_the_table(self):
        p = self.inherited()
        self.assertTrue(os.path.samestat(os.fstat(p.witness), self.realpoint.stat()))
        self.link.send(b"R"); p.poll(); p.grant(); self.assertEqual(self.link.recv(2),b"G")
        from zipworkspace.private_freeze import mount_id
        parent = mount_id(p.witness)
        identifier = b"73" if parent != b"73" else b"74"
        self.link.send(b"M" + identifier)
        self.proc.poll.return_value = 0
        p.poll()
        self.assertIn("未据此认定已经卸载",p.status())
        self.assertTrue(p.can_check_exit)
        table = b"5 1 8:1 / / rw - ext4 /dev/sda1 rw\n"
        original = table + (identifier + b" " + parent + b" 0:8 / /view\\040here ro - fuse.zip-workspace "
                            + ("zip-workspace-ro-"+p.token).encode()+b" ro\n")
        rows = []
        class NativeMetadata:
            def __init__(self, _): pass
            def read(self, name, limit):
                if name == "mountinfo": return rows.pop(0)
                with open("/proc/self/"+name, "rb") as real: return real.read(limit+1)

        with mock.patch.object(action, "ProcView", new=NativeMetadata):
            rows[:] = [original]
            self.assertIn("仍列出这笔原只读挂载", p.check_exit())
            rows[:] = [table, original]
            with self.assertRaisesRegex(Rejected, "未确认卸载"): p.check_exit()
            rows[:] = [table, table]
            self.assertIn("原目录重新可见", p.check_exit())
            self.assertEqual(rows, [])
            self.realpoint.rename(self.root/"kept-original"); self.realpoint.mkdir(mode=0o700)
            rows[:] = [table, table]
            with self.assertRaisesRegex(Rejected, "未确认卸载"): p.check_exit()
        p.close_observation()
        self.assertFalse(p.can_check_exit)
        self.proc.terminate.assert_not_called(); self.proc.kill.assert_not_called()

    @unittest.skipUnless(find_spec("PySide6"), "optional desktop extra")
    def test_qt_keeps_only_the_old_observation_after_exit_and_dispatches_no_unmount(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        win = Window(); self.addCleanup(win.close)
        pane, p = win.archive.mount, self.inherited()
        pane.set_snapshot(self.selection)
        pane.process, pane.origin = p, self.selection
        self.link.send(b"R"); p.poll(); p.grant(); self.assertEqual(self.link.recv(2), b"G")
        self.link.send(b"M73"); self.proc.poll.return_value = 0
        pane.poll()
        self.assertIsNone(pane.process)
        self.assertIs(pane.last, p)
        self.assertIn("未据此认定已经卸载", pane.result.text())
        self.assertTrue(pane.check.isEnabled())
        future, works = Future(), []
        def queue(work):
            works.append(work); return future
        with mock.patch.object(win.pool, "submit", side_effect=queue), \
             mock.patch.object(action.subprocess, "Popen", side_effect=AssertionError("no mount or unmount helper")), \
             mock.patch.object(p, "check_exit", return_value="已在原命名空间查到原目录") as observe:
            pane.check.click()
            future.set_result(works[0]()); win.poll()
        observe.assert_called_once()
        self.assertIn("查到原目录", pane.result.text())
        self.assertTrue(pane.close_window())
        self.assertIsNone(pane.last); self.assertIsNone(p.witness)

    @unittest.skipUnless(find_spec("PySide6"), "optional desktop extra")
    def test_qt_cancel_and_changed_selection_do_not_spawn_or_grant(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop import mount_pane as ui
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        with mock.patch.object(ui, "find_spec", return_value=object()): win = Window()
        self.addCleanup(win.close)
        pane = win.archive.mount
        def dispatch(title, work, done, **kw):
            f = Future()
            result = work(); f.set_result(result); done(result)
            return f
        for mode in ("cancel","selection-change"):
            pane.set_snapshot(self.selection)
            def decide(*args, **kw):
                if mode == "selection-change": pane.set_snapshot(None); return True
                return False
            with mock.patch.object(win, "job", side_effect=dispatch), mock.patch.object(ui,"confirm",side_effect=decide), \
                 mock.patch.object(ui.QFileDialog,"getExistingDirectory",return_value=self.spelling), \
                 mock.patch.object(action.subprocess,"Popen",side_effect=AssertionError("not authorized")):
                pane.choose()
            self.assertIsNone(pane.process); self.assertFalse(pane.planning)
        pane.set_snapshot(self.selection); pane.origin = self.selection; pane.process = self.inherited()
        self.link.send(b"R"); pane.set_snapshot(None); self.proc.poll.return_value = 0
        pane.poll()
        self.assertIsNone(pane.process); self.assertIn("未向助手发出挂载授权",pane.result.text())

    @unittest.skipUnless(find_spec("PySide6"), "optional desktop extra")
    def test_gui_without_mount_component_never_reaches_picker_or_native_binding(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop import mount_pane as ui
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        with mock.patch.object(ui, "find_spec", return_value=None): win = Window()
        self.addCleanup(win.close)
        pane = win.archive.mount
        pane.set_snapshot(self.selection)
        self.assertFalse(pane.pick.isEnabled())
        self.assertIn("可选挂载组件", pane.result.text())
        with mock.patch.object(ui.QFileDialog, "getExistingDirectory", side_effect=AssertionError("no component")):
            pane.choose()
        self.assertIsNone(pane.process)

    @unittest.skipUnless(find_spec("PySide6"), "optional desktop extra")
    def test_actual_window_keeps_cancel_enabled_with_ready_helper_and_an_unrelated_pending_job(self):
        from PySide6.QtWidgets import QApplication
        from zipworkspace.desktop.window import Window
        app = QApplication.instance() or QApplication([])
        win = Window(); self.addCleanup(win.close)
        win.show()
        pane = win.archive.mount
        pane.set_snapshot(self.selection); pane.origin = self.selection; pane.process = self.inherited()
        f = Future()
        with mock.patch.object(win.pool, "submit", return_value=f):
            win.job("another ordinary operation", lambda: None, lambda result: None)
        self.link.send(b"R"); pane.poll()
        self.assertFalse(win.tabs.isEnabled()); self.assertTrue(pane.process.ready)
        self.assertFalse(pane.process.sent)
        self.assertFalse(win.tabs.isAncestorOf(pane.cancel))
        self.assertTrue(win.isAncestorOf(pane.cancel))
        self.assertTrue(pane.cancel.isVisible() and pane.cancel.isEnabled())
        pane.cancel.click()  # Actual widget with its parent disabled/enabled state, not abort() directly.
        self.assertEqual(self.link.recv(2), b""); self.assertIsNotNone(win.pending)
        self.proc.poll.return_value = 0; pane.poll()
        f.set_result(None); win.poll()
        self.assertIsNone(pane.process); self.assertIsNone(win.pending)
