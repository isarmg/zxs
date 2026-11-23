"""Qt host of one first macOS full-file capture, status, or exact submit."""
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from PySide6.QtCore import QTimer, Qt
from PySide6.QtWidgets import QDialog, QFileDialog, QFormLayout, QHBoxLayout, QLineEdit, QPlainTextEdit, QVBoxLayout

from .._macos_fskit_release import require_release
from ..macos_drive_owner import _original_root
from ..macos_first_local import MacFirstLocalChoice
from ..macos_retained_edit import _on_mac
from ..macos_retained_submit import Phase, review_original, retry_original, status_original, submit_original
from ..model import Rejected
from .container_management import id_ok
from .widgets import button, confirm, label


class MacFirstDialog(QDialog):
    def __init__(self, controls, selected, cold):
        super().__init__(controls.panel)
        self.controls, self.conn, self.epoch, self.cold = controls, selected[0], selected[1], cold
        self.state = "setup"
        self.choice = self.original = self.result = self.key = self.cid = None
        self.job = self.task = None
        self.closed = self.invalidate = False
        self.pool = ThreadPoolExecutor(max_workers=1, thread_name_prefix="workspace-macos-first")
        self.setWindowTitle("核对首次提交" if cold else "首次提交本机文件")
        self.setWindowModality(Qt.WindowModality.WindowModal)
        self.resize(650, 475)
        outer = QVBoxLayout(self)
        outer.addWidget(label(f"服务器 {self.conn._exchange._location!r}　账号 {self.conn._account!r}"))
        form = QFormLayout()
        self.path, self.name, self.container = QLineEdit(), QLineEdit(), QLineEdit()
        self.path.setReadOnly(True)
        self.pick = button("选择本机文件…", self.browse)
        if cold:
            form.addRow("最初记录的新容器编号", self.container)
        else:
            form.addRow(self.pick, self.path)
            form.addRow("服务器文件名", self.name)
        outer.addLayout(form)
        self.text = QPlainTextEdit()
        self.text.setReadOnly(True)
        outer.addWidget(self.text)
        self.action, self.status = button("", self.primary), button("只查原回执", self.query)
        self.exit = button("关闭", self.reject)
        row = QHBoxLayout()
        for widget in (self.action, self.status, self.exit):
            row.addWidget(widget)
        outer.addLayout(row)
        self.timer = QTimer(self)
        self.timer.timeout.connect(self.poll)
        self.text.setPlainText("只读原编号的本机封存记录，不重新打开文件、不扫描、不自动提交。" if cold else
                               "选择一个普通本机 APFS 文件。先核对完整内容，再分别确认封存与发送。不会挂载磁盘。")
        self.paint()

    def current(self):
        current = self.controls.scope(True)
        return current is not None and current[0] is self.conn and current[1] == self.epoch

    def browse(self):
        opts = QFileDialog.Option.DontUseNativeDialog | QFileDialog.Option.ReadOnly | QFileDialog.Option.DontResolveSymlinks
        path, _ = QFileDialog.getOpenFileName(self, "选择原本机普通文件", options=opts)
        if path and self.state == "setup" and self.job is None and self.current():
            self.path.setText(path)
            self.name.setText(Path(path).name)

    def paint(self):
        names = {"setup": "读取原记录" if self.cold else "核对文件", "reviewed": "准备首次提交…",
                 "ready": "提交完整文件…", "absent": "发送同一笔原提交…"}
        idle, setup = self.job is None, self.state == "setup"
        self.action.setText(names.get(self.state, "等待"))
        self.action.setEnabled(idle and self.state in names)
        self.status.setEnabled(idle and self.original is not None and self.state != "done")
        self.exit.setEnabled(idle)
        for widget in (self.pick, self.path, self.name, self.container):
            widget.setEnabled(idle and setup)

    def work(self, task, fn):
        if self.job is not None or not self.current():
            return
        self.task, self.job = task, self.pool.submit(fn)
        self.timer.start(150)
        self.paint()

    def recover(self):
        require_release()
        root = _on_mac(_original_root, False)
        return review_original(self.conn, root, self.cid, "main", None, confirmed=True)

    def describe(self):
        a = self.original.action
        return (f"新容器 {a.name!r} ({a.container_id})，main 首版\n"
                f"{a.body.length:,} 字节　SHA-256 {a.body.sha256}\n"
                f"服务器 {self.conn._exchange._location!r}　账号 {self.conn._account!r}\n"
                f"新节点 {a.node_id}\n原提交 {self.original.key.submission}\n原操作 {a.operation_id}")

    def primary(self):
        if self.job is not None or not self.current():
            return
        if self.state == "setup":
            if self.cold:
                cid = self.container.text().strip()
                if not id_ok(cid):
                    self.text.setPlainText("请输入原容器的 32 位编号。")
                    return
                self.cid = cid
                self.work("recover", self.recover)
            else:
                path, name = self.path.text(), self.name.text()
                if path and name:
                    def choose():
                        require_release()
                        return MacFirstLocalChoice(self.conn, path, name)
                    self.work("open", choose)
        elif self.state == "reviewed":
            r = self.choice.review
            text = (f"原文件 {r.path!r}\n服务器 {r.origin!r}　账号 {r.selected.account!r}\n"
                    f"新容器 {r.selected.container.name!r} ({self.cid})，main 首版\n"
                    f"{r.body.length:,} 字节　SHA-256 {r.body.sha256}\n"
                    "本步只封存在原本机私有状态中，不发送。请保留容器编号。")
            if confirm(self, "封存本机文件", text, attestation="确认保留这份完整本机原件的待提交副本。",
                       action_text="封存完整文件"):
                def keep():
                    require_release()
                    root = _on_mac(_original_root, True)
                    try:
                        key = self.choice.capture(r, root, confirmed=True)
                    finally:
                        self.choice.close()
                    original = review_original(self.conn, root, self.cid, "main", None, confirmed=True)
                    if original.key != key:
                        raise Rejected("未找到同一笔首次封存。请用原容器编号核对。")
                    return original
                self.work("prepare", keep)
        elif self.state in ("ready", "absent"):
            if not confirm(self, "提交首次完整文件", self.describe(), attestation="确认向上述服务器提交这份完整正文。",
                           action_text="按原号提交"):
                return
            absent = self.result if self.state == "absent" else None
            def send():
                require_release()
                if absent is not None and absent.attempted:
                    return retry_original(self.conn, absent, confirmed=True)
                return submit_original(self.conn, self.original, confirmed=True)
            self.work("send", send)

    def query(self):
        if self.original is None:
            return
        def status():
            require_release()
            return status_original(self.conn, self.original, confirmed=True)
        self.work("status", status)

    def poll(self):
        if self.job is None or not self.job.done():
            return
        self.timer.stop()
        job, task = self.job, self.task
        self.job = self.task = None
        try:
            answer = job.result()
            if task == "open":
                if not self.current():
                    answer.close()
                    raise Rejected("连接已改变，未保留原文件。")
                self.choice = answer
                r = answer.review
                self.cid = r.selected.container.id
                self.state = "reviewed"
                message = (f"新容器编号 {self.cid}\n{r.body.length:,} 字节　SHA-256 {r.body.sha256}\n"
                           "尚未封存或发送。")
            elif task in ("recover", "prepare"):
                self.original = answer
                self.state = "pending" if task == "recover" else "ready"
                message = ("只查原回执；不要猜测是否曾经发送。" if task == "recover" else "已封存；尚未发送。")
                message += "\n" + self.describe()
            else:
                self.result = answer
                if answer.phase is Phase.CONFIRMED:
                    self.state, self.invalidate = "done", True
                    message = "服务器已确认首次提交，原回执已留在本机。"
                elif answer.phase is Phase.ABSENT and task == "status":
                    self.state = "absent"
                    message = "服务器明确未找到原回执。可按原号发送同一份正文。"
                else:
                    self.state = "pending"
                    message = ("服务器报告冲突，未替换现有容器。" if answer.phase is Phase.CONFLICT else
                               "原任务结果未知。只查同一原号，不要另起一笔。")
                message += "\n" + self.describe()
        except Exception as error:
            self.state = "setup" if task in ("open", "recover") else "pending"
            message = f"本步未完成：{error}"
            if self.cid is not None:
                message += f"\n保留原容器编号 {self.cid}，只核对这笔原记录。"
        self.text.setPlainText(message)
        self.paint()

    def reject(self):
        if self.job is not None or self.closed:
            return
        self.closed = True
        try:
            if self.choice is not None:
                self.choice.close()
        finally:
            self.pool.shutdown(wait=False)
            super().reject()

    def closeEvent(self, event):
        if self.closed:
            event.accept()
        else:
            event.ignore()
            self.reject()
