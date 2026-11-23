"""Windows R3 desktop workflow. No implicit upload on mount, last Close, dialog exit or reconnect."""
from concurrent.futures import ThreadPoolExecutor
from functools import partial
import re
from threading import Event, Lock

from PySide6.QtCore import QTimer, Qt
from PySide6.QtWidgets import (QDialog, QFileDialog, QFormLayout, QHBoxLayout, QLineEdit,
                               QPlainTextEdit, QVBoxLayout, QWidget)

from .._windows_provider import admitted_client, admitted_provider, packaged
from ..model import Rejected
from .container_management import Phase
from .container_windows_draft import WindowsDraftChoice
from .container_windows_fileplane import _alias
from .container_windows_mount import WindowsPrivateDraftMount
from .container_windows_receipt import WindowsReceiptUncertain, lookup_original_windows_receipt
from .container_windows_retire import WindowsRetirementPending, retire_original_windows_save
from .container_windows_save import WindowsSaveKey, WindowsSaveUncertain, _key_slot, _slot, load_original_windows_key
from .container_windows_submit import WindowsOriginalSaveChoice
from .widgets import button, confirm, label


def local_alias(name):
    value = _alias(name)
    if len((value + ":Zone.Identifier:$DATA").encode("utf-16-le")) > 510:
        raise Rejected("本机名称过长，无法保留来源数据流。")
    return value


def display_key(key):
    return " ".join((key.container, key.submission, key.operation, key.record_sha256))


class WindowsDiskControls(QWidget):
    def __init__(self, host, panel):
        super().__init__(panel)
        self.host, self.panel, self.active = host, panel, None
        self._known = {}  # Exact observed server effect whose local retention/release was uncertain.
        layout = QVBoxLayout(self)
        self.note = label("Windows 原节点磁盘：本机编辑；只有明确提交才发送完整文件。" if packaged()
                          else "本版本没有已签名且通过 Windows 原生验收的磁盘组件。仍可浏览远端容器。")
        row = QHBoxLayout()
        self.edit = button("将分支头作为本机磁盘打开…", lambda: self.open(False))
        self.recover = button("核对待处理的原提交…", lambda: self.open(True))
        row.addWidget(self.edit)
        row.addWidget(self.recover)
        layout.addWidget(self.note)
        layout.addLayout(row)

    def _scope(self, recovery):
        p, owner = self.panel, self.panel._owned
        cid = p._current()
        if (not packaged() or owner is None or owner.writable or not owner._listed or owner.operations is not p.ops
                or p._checking_connection or p.jobs or self.host.pending is not None
                or self.host._container_edit_locked or cid not in p.listed):
            return None
        if recovery:
            return owner, p._epoch, cid, None, None, None
        info, br, node = p.details.get(cid), p.branches.currentData(), p.nodes.currentData()
        if (info is None or info.container != p.listed[cid] or not info.container.revision or br is None
                or br not in info.branches or node is None or node not in br.nodes or node.id != br.head):
            return None
        return owner, p._epoch, cid, info, br, node

    def paint(self):
        self.edit.setEnabled(not self.busy() and self._scope(False) is not None)
        self.recover.setEnabled(not self.busy() and self._scope(True) is not None)

    def busy(self):
        return self.active is not None

    def open(self, recovery):
        selected = self._scope(recovery)
        if self.busy() or selected is None:
            return
        box = self.active = _Disk(self, selected, recovery)
        self.panel._paint()
        def finished(_):
            if self.active is box:
                self.active = None
                if box.invalidate:
                    self.panel.details.pop(selected[2], None)
                    self.panel._select()
                self.panel._paint()
            box.deleteLater()
        box.finished.connect(finished)
        box.open()

    def finish(self):
        if self.busy():
            raise Rejected("先完成关闭 Windows 原生磁盘窗口。")


class _Capture:
    """Worker owns the draft/host. UI observes published pointers; only stop/abandon cross threads."""
    def __init__(self):
        self.lock, self.owner, self.path, self.draft = Lock(), None, None, None
        self.cancelled, self.seed = Event(), None

    def live(self):
        with self.lock:
            return self.owner, self.path

    def cancel_acquisition(self):
        # Never run handle cleanup, disk or network on the GUI thread.
        with self.lock:
            if self.owner is not None:
                return False
            self.cancelled.set()
            if self.seed is not None:
                self.seed.cancel_acquisition()
        return True

    def _check(self):
        if self.cancelled.is_set():
            raise Rejected("已取消打开；没有提交。")

    def run(self, selected, folder, alias, limit):
        self._check()
        admitted_provider()  # Refuse a missing/untrusted packaged component before writing or downloading.
        self._check()
        conn, _, _, info, branch, node = selected
        choice = WindowsDraftChoice(conn, info, branch, node, folder, max_bytes=limit)
        with self.lock:
            self.path, self.seed = choice.local_file, choice
        try:
            self._check()
            # Advisory before download, from the same held private root. Durable no-replace
            # capture still arbitrates another instance appearing after these two checks.
            src, home = choice.source, choice._home
            try:
                for slot in (_slot, _key_slot):
                    home.api.absent(home.handle, slot(src.destination, src.account, src.container.id))
            except FileExistsError as error:
                raise Rejected("该私有目录中已有此容器的原提交。请先使用「核对待处理的原提交」。") from error
            draft = self.draft = choice.begin(confirmed=True)
            self._check()
            owner = WindowsPrivateDraftMount(draft, alias, confirmed=True)
            with self.lock:
                self._check()
                self.owner, self.seed = owner, None
            try:
                result = owner.run(allow_installed_driver_start=True, allow_directory_mount=True)
            finally:
                owner.release_fault()
            if (result.get("outcome") != "original-detached" or not result.get("drained") or
                    not result.get("absent") or not result.get("presented")):
                raise Rejected("没有取得正常卸载和全部关闭的原件证明：" + str(result.get("outcome")))
            return draft.review(confirmed=True)
        finally:
            with self.lock:
                self.seed = None
            choice.close()

    def close(self):
        if self.owner is not None:
            self.owner.release_fault()
        if self.draft is not None:
            self.draft.close()
            self.draft = None


class _Disk(QDialog):
    def __init__(self, controls, selected, recovery):
        super().__init__(controls.panel)
        self.controls, self.selected, self.recovery = controls, selected, recovery
        self.conn, _, self.cid, info, branch, node = selected
        self.setWindowTitle("核对 Windows 原提交" if recovery else "Windows 原生工作磁盘")
        self.setWindowModality(Qt.WindowModality.WindowModal)
        self.resize(650, 550)
        self.pool = ThreadPoolExecutor(max_workers=1, thread_name_prefix="workspace-windows-disk")
        self.job = self.task = self.key = self.review = self.choice = self.decision = None
        self.capture, self.state, self.invalidate = _Capture(), "setup", False
        self.done_closing, self.matched = False, None
        self.site = self.conn._exchange._location
        layout = QVBoxLayout(self)
        detail = f"服务器 {self.site!r}　账号 {self.conn._account!r}\n容器 {self.cid}"
        if info:
            detail += (f"　{info.container.name!r}　修订 {info.container.revision}\n"
                       f"分支 {branch.id!r}　原节点 {node.id}　{node.length:,} 字节")
        layout.addWidget(label(detail))
        form = QFormLayout()
        self.folder = QLineEdit()
        self.folder.setReadOnly(True)
        self.browse = button("选择现有私有 NTFS 目录…", self._browse)
        form.addRow(self.browse, self.folder)
        self.alias, self.limit, self.prior = QLineEdit(), QLineEdit(), QLineEdit()
        if recovery:
            self.prior.setPlaceholderText("留空读取该容器的活动原号；已归档时粘贴四段原号")
            form.addRow("原传输信息", self.prior)
        else:
            try:
                alias = local_alias(info.container.name)
            except Rejected:
                alias = "文件-" + self.cid[:8]
            self.alias.setText(alias)
            self.limit.setText(str(max(64 << 20, node.length)))
            form.addRow("仅本机名称（不改变服务器名称）", self.alias)
            form.addRow("本次文件大小上限（字节）", self.limit)
        layout.addLayout(form)
        self.info = QPlainTextEdit()
        self.info.setReadOnly(True)
        layout.addWidget(self.info)
        self.action = button("读取原提交" if recovery else "打开本机磁盘…", self.primary)
        self.status = button("只查原回执", self.query)
        self.force = button("强制结束…", self.abandon)
        self.exit = button("关闭", self.reject)
        row = QHBoxLayout()
        for btn in (self.action, self.status, self.force, self.exit):
            row.addWidget(btn)
        layout.addLayout(row)
        self.note("不会扫描目录或自动重试。核对回执无需打开原正文。" if recovery else
                  "只呈现所选分支头的一个文件。使用能原位读写的编辑器；当前不支持新建临时文件后替换、重命名或执行程序。\n"
                  "选择自己的普通本机私有 NTFS 子目录，不要用同步目录、联接或共享目录。关闭编辑器和该目录的资源管理器窗口，再正常停止。只有之后点击提交才会上传。")
        self.timer = QTimer(self)
        self.timer.timeout.connect(self.poll)
        self.paint()

    def note(self, message):
        if self.info.toPlainText() != message:
            self.info.setPlainText(message)

    def current(self):
        current = self.controls._scope(self.recovery)
        return current is not None and current[0] is self.selected[0] and current[1:] == self.selected[1:]

    def _browse(self):
        opts = (QFileDialog.Option.ShowDirsOnly | QFileDialog.Option.DontUseNativeDialog |
                QFileDialog.Option.ReadOnly | QFileDialog.Option.DontResolveSymlinks)
        path = QFileDialog.getExistingDirectory(self, "选择原本机私有 NTFS 目录", self.folder.text(), options=opts)
        if path and self.current() and self.state == "setup":
            self.folder.setText(path)
            self.paint()

    def work(self, task, fn):
        if self.job is not None:
            return
        self.task, self.job = task, self.pool.submit(fn)
        self.timer.start(150)
        self.paint()

    def paint(self):
        task, state = self.task, self.state
        idle = self.job is None
        names = {"setup": "读取原提交" if self.recovery else "打开本机磁盘…", "mount": "正常停止并核对修改",
                 "reviewed": "准备完整文件提交", "prepared": "提交完整文件…", "absent": "发送同一笔原提交",
                 "settled": "归档本地待处理记录"}
        self.action.setText(names.get(state, "等待"))
        owner, _ = self.capture.live()
        stopping = state in ("stopping", "abandoned", "cancelling")
        self.action.setEnabled((idle and state in names and state != "mount" and bool(self.folder.text()))
                               or (task == "mount" and state == "mount" and owner is not None and owner.wait_location(0) is not None))
        self.status.setEnabled(idle and self.key is not None and state not in ("closed", "closing"))
        self.force.setText("取消打开或下载" if owner is None else "强制结束…")
        self.force.setVisible(task == "mount" and state not in ("abandoned", "cancelling"))
        self.force.setEnabled(not self.done_closing)
        self.exit.setEnabled(idle and not stopping)
        for w in (self.browse, self.alias, self.limit, self.prior):
            w.setEnabled(idle and state == "setup")

    def primary(self):
        if self.task == "mount":
            owner, _ = self.capture.live()
            if owner is not None and self.state == "mount":
                owner.stop()
                self.state = "stopping"
                self.paint()
            return
        if self.job is not None or not self.current():
            return
        try:
            if self.state == "setup":
                self.begin()
            elif self.state == "reviewed":
                self.work("prepare", self.prepare)
            elif self.state in ("prepared", "absent"):
                self.send()
            elif self.state == "settled":
                self.work("retire", partial(retire_original_windows_save, self.conn, self.directory,
                                            self.key, confirmed=True))
        except Exception as error:
            self.note(str(error))
            self.paint()

    def begin(self):
        folder = self.folder.text()
        if not folder:
            return
        self.directory = folder
        if self.recovery:
            prior = self.prior.text().strip()
            def read():
                admitted_client()  # Does not require the vendor service, driver, live body or mount.
                if prior:
                    fields = prior.split()
                    if len(fields) != 4 or fields[0] != self.cid:
                        raise Rejected("需要同一容器的原四段传输信息。")
                    key = WindowsSaveKey(*fields)
                else:
                    key = load_original_windows_key(self.conn, folder, self.cid, confirmed=True)
                try:
                    return key, WindowsOriginalSaveChoice(self.conn, folder, key)
                except Exception as error:
                    raise WindowsSaveUncertain(key, folder) from error
            self.work("recover", read)
            return
        alias = local_alias(self.alias.text())
        raw, node = self.limit.text(), self.selected[-1]
        if not re.fullmatch(r"[0-9]{1,19}", raw) or not node.length <= int(raw) <= (1 << 63)-4096:
            raise Rejected("输入能容纳原文件的明确字节上限。")
        text = (f"服务器 {self.site!r}　账号 {self.conn._account!r}\n容器 {self.cid}，分支 {self.selected[-2].id!r}\n"
                f"原节点 {node.id}\n本机目录 {folder!r}\n磁盘内的名称 {alias!r}，最多 {int(raw):,} 字节。\n"
                "下载此原节点；Windows 可能启动已安装的 WinFsp 文件系统服务，并在所选目录创建新的私有目录挂载点。"
                "不会安装驱动、占用盘符或自动提交。")
        if confirm(self, "打开所选原节点", text, attestation="我选择这个原节点和私有目录，并允许本次服务启动及挂载。",
                   action_text="下载并打开") and self.current():
            self.state = "mount"
            self.note("正在核对签名、原服务与私有位置，再读取所选分支头。")
            self.work("mount", partial(self.capture.run, self.selected, folder, alias, int(raw)))

    def prepare(self):
        key = self.capture.draft.prepare_save(self.review, confirmed=True)
        try:
            return key, WindowsOriginalSaveChoice(self.conn, self.directory, key)
        except Exception as error:
            raise WindowsSaveUncertain(key, self.directory) from error

    def describe(self):
        src, ticket = self.choice.review.source, self.choice.review.ticket
        body = ticket.change.body
        return (f"服务器 {src.origin!r}　账号 {src.account!r}\n容器 {src.container.name!r} ({src.container.id})，"
                f"原修订 {ticket.base_sequence}\n分支 {src.branch!r}\n原节点 {src.head.id}\n新节点 {body.node}\n"
                f"完整正文 {body.length:,} 字节　SHA-256 {body.sha256}\n原传输信息（可复制，不含口令）：\n"
                + display_key(self.key))

    def send(self):
        choice = self.choice
        if self.matched is not None or self.key in self.controls._known:
            return  # Once observed, absence in another response does not undo the original remote effect.
        if not choice or self.decision != choice.review:
            if not choice or not confirm(self, "提交完整文件", self.describe(), attestation="确认将这份完整正文提交到上述原容器和分支。",
                                         action_text="核对原回执并提交") or not self.current():
                return
            self.decision = choice.review
        # A retry creates no new operation and needs no duplicate confirmation for the same decision.
        def apply():
            admitted_client()
            selected = WindowsOriginalSaveChoice(self.conn, self.directory, self.key)
            if selected.review != self.decision:
                raise Rejected("已经确认的原提交详情发生变化。")
            return selected.apply(selected.review, confirmed=True)
        self.note("正在核对原回执。只有服务器明确没有该原号的回执，才会发送原完整正文。")
        self.work("send", apply)

    def query(self):
        if self.job is None and self.key is not None and self.current():
            def read():
                admitted_client()
                result = lookup_original_windows_receipt(self.conn, self.directory, self.key, confirmed=True)
                choice = (WindowsOriginalSaveChoice(self.conn, self.directory, self.key)
                          if result.phase is Phase.ABSENT and self.matched is None else None)
                return result, choice
            self.work("query", read)

    def abandon(self):
        owner, _ = self.capture.live()
        job = self.job
        if self.task != "mount" or job is None or job.done():
            return
        if owner is None:
            if self.capture.cancel_acquisition():
                self.state = "cancelling"
                self.note("已请求取消。等待当前本地读取或网络请求返回；不会提交。")
                self.paint()
            return
        if confirm(self, "强制结束原磁盘",
                "编辑器、映射或系统缓存中尚未交回的修改可能丢失。保留已写入的私有原件；本次不会提交。",
                attestation="放弃正常卸载证明，不把这次会话作为可提交的修改。", action_text="强制结束"):
            # A nested Qt consent loop can deliver the completed worker before returning here.
            if self.task != "mount" or self.job is not job or job.done() or self.capture.live()[0] is not owner:
                return
            if owner.abandon(confirmed=True):
                self.state = "abandoned"
            self.paint()

    def _tail(self):
        _, path = self.capture.live()
        key = "\n原传输信息（可复制）：\n" + display_key(self.key) if self.key is not None else ""
        return key + ("\n私有文件原计划位置（若已创建则保留）：" + path if path else "")

    def poll(self):
        job, task = self.job, self.task
        if job is None:
            return
        if not job.done():
            if task == "mount":
                owner, _ = self.capture.live()
                if owner is not None:
                    visible = owner.wait_location(0)
                    if self.state == "mount" and visible:
                        text = ("磁盘已就绪。在资源管理器中粘贴下面的目录（不会自动运行任何文件）：\n" + visible +
                                "\n本机文件名：" + repr(self.alias.text()) +
                                "\n编辑并关闭文件后点击「正常停止并核对修改」。现在没有上传。")
                        if self.info.toPlainText() != text:
                            self.note(text)
                    elif self.state in ("stopping", "abandoned"):
                        remain = owner.remaining
                        self.note(("已停止接收新打开。等待编辑器、资源管理器窗口等全部关闭并验证原卷已撤下。"
                                   if self.state == "stopping" else "正在强制结束；不会取得可提交证明。")
                                  + (f"\n尚待关闭的原引用：{remain}" if type(remain) is int else ""))
                    self.paint()
            return
        self.timer.stop()
        self.job = self.task = None
        try:
            result = job.result()
            if task == "close":
                self.done_closing = True
                self.pool.shutdown(wait=False)
                super().reject()
                return
            if task == "mount":
                self.review = result
                self.state = "reviewed" if result.changed else "unchanged"
                self.note(("已正常卸载，发现本地修改。准备提交只会封存字节和原号，不访问服务器。"
                           if result.changed else "已正常卸载，完整正文与原节点一致，无需上传。")
                          + f"\n{result.length:,} 字节　SHA-256 {result.sha256}" + self._tail())
            elif task in ("prepare", "recover"):
                self.key, self.choice = result
                self.matched = self.controls._known.get(self.key)
                self.state = "prepared" if task == "prepare" else "pending"
                self.note(("本机已封存原意图；尚未提交。\n" if task == "prepare" else
                           "已找到原意图。先只查原回执；不推定先前是否已经发送。\n") + self.describe())
            elif task in ("send", "query"):
                if task == "query":
                    result, choice = result
                    if choice is not None:
                        self.choice = choice
                phase = result.phase
                if phase is Phase.CONFIRMED:
                    if self.matched is not None and self.matched != result.receipt:
                        raise Rejected("后来的回执与已核同一原号的回执不一致；只核对。")
                    self.matched = result.receipt
                    if task == "send":
                        self.controls._known.pop(self.key, None)  # Durable receipt + successful, completed retirement.
                    else:
                        self.controls._known[self.key] = self.matched
                    self.invalidate = True
                    self.state = "settled" if task == "query" else "complete"
                    text = (f"服务器已确认这笔原操作。分支 {result.receipt.branch!r}，新节点 {result.receipt.node}，"
                            f"修订 {result.receipt.revision}。")
                    text += ("可以在本机归档原记录并释放待处理位。" if task == "query" else "原本机待处理位也已归档。")
                elif self.matched is not None:
                    self.state = "reconcile"
                    text = (f"此前已经核实服务器修订 {self.matched.revision}。本次未返回相同回执；"
                            "只核对原号，不能因后来的缺席、冲突或未知而再次发送。")
                elif phase is Phase.ABSENT and task == "query":
                    self.state = "absent"
                    text = "服务器明确没有该原号回执。可选择发送同一笔；发送前仍会再次核对。"
                elif phase is Phase.CONFLICT:
                    self.state, self.invalidate = "conflict", True
                    text = "原容器修订或分支头冲突。本次不改用新编号、合并或覆盖。"
                else:
                    self.state = "pending"
                    text = "原结果未知。不要新建提交；只核对同一原号。"
                self.note(text + self._tail())
            elif task == "retire":
                self.controls._known.pop(self.key, None)
                self.state, self.invalidate = "complete", True
                self.note(f"已归档同一笔本机记录。已核服务器修订 {result.revision}。" + self._tail())
        except (WindowsReceiptUncertain, WindowsRetirementPending) as error:
            self.key, self.invalidate = error.key, True
            self.matched = self.controls._known.setdefault(error.key, error.receipt)
            self.state = "settled" if isinstance(error, WindowsRetirementPending) else "pending"
            self.note(f"已收到精确匹配的服务器回执（修订 {error.receipt.revision}），不要重复提交。"
                      + ("只需完成本机归档。" if self.state == "settled" else "本机保留回执尚未确认；只查原号。")
                      + self._tail())
        except WindowsSaveUncertain as error:
            self.key, self.state = error.key, "pending"
            self.matched = self.controls._known.get(self.key)
            self.note("已取得原传输号，但还不能证明完整原意图可用，请保留原私有目录和下面的信息。不要重新准备。" + self._tail())
        except Exception as error:
            self.state = ("pending" if self.key else "setup" if task == "recover" else "fault")
            self.note(f"本步未完成：{error}\n" +
                      ("只核对原号；此错误不能证明上次没有上传。" if self.key else
                       "未由此错误取得可提交证明。不要移动或删除私有原件。") + self._tail())
        self.paint()

    def reject(self):
        if self.done_closing:
            super().reject()
        elif self.job is None:
            if self.state == "reviewed" and not confirm(self, "保留本地修改并关闭",
                    "还没有封存原提交。私有文件会保留，但关闭后不能凭路径恢复本次挂载的提交权限。" + self._tail(),
                    attestation="我已记录原件位置，本次不准备提交。", action_text="保留原件并关闭"):
                return
            self.work("close", self.capture.close)

    def closeEvent(self, event):
        if self.done_closing:
            event.accept()
        else:
            event.ignore()
            self.reject()
