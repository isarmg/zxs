"""The Linux desktop controls one explicit, independently served read-only snapshot."""
from PySide6.QtCore import QTimer
from PySide6.QtWidgets import QApplication, QFileDialog, QHBoxLayout, QVBoxLayout, QWidget

from .mount_action import MountChoice
from .widgets import button, confirm, label


class MountPane(QWidget):
    def __init__(self, owner):
        super().__init__()
        self.owner, self.selection = owner, None
        self.planning, self.process, self.origin = False, None, None
        self.timer = QTimer(self)
        self.timer.timeout.connect(self.poll)
        layout = QVBoxLayout(self)
        layout.addWidget(label("把当前打开的固定快照挂到已有私有空目录。此操作不能写入，也不会跟随分支头。"))
        actions = QHBoxLayout()
        self.pick = button("选择挂载点并预览…", self.choose)
        actions.addWidget(self.pick)
        layout.addLayout(actions)
        # Window puts this strip outside the tab stack that unrelated background tasks disable.
        self.banner = QWidget()
        bar = QHBoxLayout(self.banner)
        self.banner_note = label()
        self.cancel = button("取消只读挂载的归档核验", self.abort)
        bar.addWidget(self.banner_note); bar.addWidget(self.cancel)
        self.banner.hide()
        self.result = label("尚未请求挂载。")
        layout.addWidget(self.result)
        layout.addStretch()
        self.buttons()

    def buttons(self):
        p = self.process
        self.pick.setEnabled(not self.planning and p is None and self.selection is not None
                             and self.selection.held is not None and not self.selection.held.closed)
        can_cancel = p is not None and not p.sent and not p.cancelled and not p.exited
        self.cancel.setEnabled(can_cancel)
        if can_cancel:
            state = p.status()
            if self.banner_note.text() != state: self.banner_note.setText(state)
        self.banner.setVisible(can_cancel)

    def abort(self):
        if self.process is not None and self.process.cancel():
            self.result.setText(self.process.status()); self.buttons()

    def changing(self):
        # Invalidate as soon as a new selection is requested, including a nested Qt picker.
        if self.process is not None and not self.process.sent: self.abort()

    def set_snapshot(self, selection):
        if selection is not self.selection: self.changing()
        self.selection = selection
        self.buttons()

    def choose(self):
        s = self.selection
        if s is None or self.planning or self.process is not None or self.owner.pending is not None: return
        self.planning = True
        self.buttons()
        plan = None
        def end():
            if plan is not None: plan.close()
            self.planning = False; self.buttons()
        def failed(err):
            try: self.result.setText(str(err))
            finally: end()
        def preview(detail):
            try:
                if s is not self.selection or not confirm(self, "将一个固定快照挂为只读目录", detail,
                        attestation="从现在到卸载，我独占此空目录及其名称，不移动、同步、替换或另行挂载。",
                        action_text="核验并挂载此版本") or s is not self.selection:
                    self.result.setText("已取消；没有启动挂载助手。")
                    return
                self.process = plan.launch(confirmed=True)
                self.origin = s
                self.timer.start(120)
                self.result.setText(self.process.status())
            finally: end()
        try:
            opts = QFileDialog.Option.ShowDirsOnly | QFileDialog.Option.DontResolveSymlinks
            name = QFileDialog.getExistingDirectory(self, "选择原有的本人 0700 私有空挂载点", "", opts)
            if not name or s is not self.selection:
                end(); return
            plan = MountChoice(s, name)
            work = self.owner.job("正在只读核对原件和所选空目录，尚未请求挂载…", plan.review, preview, failed=failed)
            if work is None: end()
            else: work.add_done_callback(lambda f: plan.close() if f.cancelled() else None)
        except Exception as err:
            failed(err)

    def poll(self):
        p = self.process
        if p is None: return
        p.poll()
        if p.ready and not (p.sent or p.cancelled or p.exited or p.error):
            if self.selection is not self.origin: p.cancel()
            elif self.owner.pending is None and QApplication.activeModalWidget() is None:
                try: p.grant()  # The same explicit confirmation; do not silently authorize a new selection.
                except (ValueError, OSError): p.cancel()
        status = p.status()
        if self.result.text() != status: self.result.setText(status)
        if p.exited:
            self.timer.stop()
            self.process = self.origin = None
        self.buttons()

    def close_window(self):
        if self.planning:
            self.owner.inform("请先关闭挂载预览。")
            return False
        p = self.process
        if p is None: return True
        p.poll()
        if p.sent and not p.exited:
            if not confirm(self, "只关闭窗口，不卸载", p.instructions() + "\n\n窗口关闭本身不停止助手；也不证明挂载仍在或已卸载。",
                           attestation="我已记下核验和正常卸载的方法；关闭窗口可能留下仍在服务的挂载。",
                           action_text="仅关闭窗口"):
                return False
        self.timer.stop()
        p.leave()  # Before G: cancel. After G: never kill the server or unmount a reused pathname.
        self.process = self.origin = None
        return True
