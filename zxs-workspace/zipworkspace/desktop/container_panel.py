"""Workspace's in-window HTML management page and native action controls."""

from concurrent.futures import ThreadPoolExecutor
from dataclasses import replace
from functools import partial
import json
import sys

from PySide6.QtCore import Qt, QTimer
from PySide6.QtWidgets import (QComboBox, QDialog, QDialogButtonBox, QFormLayout, QHBoxLayout,
                               QInputDialog, QLineEdit, QListWidget, QListWidgetItem, QSplitter,
                               QTextBrowser, QVBoxLayout, QWidget)

from ..model import new_id
from .container_client import ContainerIndex, index_for
from .container_connection import open_connection
from .container_management import (CreateBranch, Kind, Phase, Recycle, Rename, Tags, branch_choice,
                                   branch_source, kind, lookup, open_checked, page, perform, retry,
                                   save_choice, save_mode, tags_ok, text_ok, unresolved)
from .widgets import button, confirm, label

_PARALLEL = 4
_ID = Qt.ItemDataRole.UserRole


class _LocalPage(QTextBrowser):
    def __init__(self):
        super().__init__()
        self.setOpenLinks(False)
        self.setOpenExternalLinks(False)
        self.setAccessibleName("容器管理详情")
        self.setStyleSheet("QTextBrowser { border: 1px solid palette(mid); border-radius: 12px; padding: 8px; }")

    def loadResource(self, _kind, _url):
        return None  # all markup comes from page(); never load a URL, image, or other resource


class ContainerPanel(QWidget):
    def __init__(self, operations=None, *, write_profile=None, native_window=None):
        super().__init__()
        self.ops, self.editor, self.cold = operations, None, None
        self._configured_profile = self._profile = write_profile
        self._owned = None
        self._checking_connection = False
        self._epoch = 0
        self.pool = None
        self.jobs = {}
        self.listed, self.items, self.details, self.outcomes, self.errors = {}, {}, {}, {}, {}
        self.recovery = set()
        self._empty_observed = False
        self.timer = QTimer(self)
        self.timer.timeout.connect(self._poll)

        outer = QVBoxLayout(self)
        top = QHBoxLayout()
        top.addWidget(label("存储容器"))
        self.connect_button = button("连接…", self._connect)
        self.disconnect_button = button("断开连接", self._disconnect)
        self.refresh = button("读取列表", self._load)
        top.addWidget(self.connect_button)
        top.addWidget(self.disconnect_button)
        top.addWidget(self.refresh)
        top.addStretch()
        outer.addLayout(top)
        self.notice = label("尚未连接 Workspace 存储客户端。" if operations is None else "读取列表后，选择一个容器。")
        outer.addWidget(self.notice)
        panels = QSplitter()
        outer.addWidget(panels, 1)

        left = QWidget()
        l = QVBoxLayout(left)
        self.entries = QListWidget()
        self.entries.setAccessibleName("容器与各自的原操作状态")
        self.entries.setAlternatingRowColors(True)
        self.entries.currentItemChanged.connect(lambda *_: self._select())
        self.entries.itemActivated.connect(lambda *_: self._open())
        l.addWidget(self.entries)
        self.open_button = button("打开容器信息", self._open)
        l.addWidget(self.open_button)
        panels.addWidget(left)

        right = QWidget()
        r = QVBoxLayout(right)
        self.browser = _LocalPage()
        r.addWidget(self.browser, 1)
        choices = QFormLayout()
        self.branches = QComboBox()
        self.branches.currentIndexChanged.connect(lambda *_: self._pick_branch())
        self.nodes = QComboBox()
        self.nodes.currentIndexChanged.connect(lambda *_: self._paint())
        choices.addRow("分支", self.branches)
        choices.addRow("节点", self.nodes)
        r.addLayout(choices)
        actions = QFormLayout()
        self.rename_button = button("重命名…", self._rename)
        self.tags_button = button("替换标签…", self._tags)
        self.save_button = button("保存该容器的新节点…", self._save)
        self.branch_button = button("从所选节点新建分支…", self._create_branch)
        self.recycle_button = button("删除到回收盘…", self._recycle)
        self.status_button = button("查询原回执", self._status)
        self.retry_button = button("重试这笔原操作…", self._retry)
        actions.addRow(self.rename_button, self.tags_button)
        actions.addRow(self.save_button, self.branch_button)
        actions.addRow(self.recycle_button, self.status_button)
        actions.addRow(self.retry_button)
        self.copy_transfer_button = None
        if sys.platform == "linux":
            self.copy_transfer_button = button("复制原传输信息", self._copy_selected_transfer)
            actions.addRow(self.copy_transfer_button)
        r.addLayout(actions)
        panels.addWidget(right)
        panels.setStretchFactor(1, 2)
        if sys.platform == "linux" and native_window is not None:
            from .container_edit_controls import ContainerEditControls
            self.editor = ContainerEditControls(native_window, self)
            outer.addWidget(self.editor)
        if sys.platform == "linux":
            from .container_cold_controls import ColdTransferControls
            self.cold = ColdTransferControls(self)
            outer.addWidget(self.cold)
        self._paint()

    def busy(self):
        return bool(self.jobs)

    def finish(self):
        self.timer.stop()
        if self.editor is not None:
            self.editor.finish()
        if self.cold is not None:
            self.cold.finish()
        if self._owned is not None:
            self._owned.revoke()
        if self.pool is not None:
            self.pool.shutdown(wait=False)

    def _connect(self):
        if self.ops is not None or self.jobs or (self.editor is not None and self.editor.busy()):
            return
        dialog = QDialog(self)
        dialog.setWindowTitle("连接存储客户端")
        layout = QFormLayout(dialog)
        origin, login, password = QLineEdit(), QLineEdit(), QLineEdit()
        origin.setPlaceholderText("https://")
        password.setEchoMode(QLineEdit.EchoMode.Password)
        password.setInputMethodHints(Qt.InputMethodHint.ImhHiddenText | Qt.InputMethodHint.ImhSensitiveData
                                     | Qt.InputMethodHint.ImhNoPredictiveText)
        layout.addRow("HTTPS 地址", origin)
        layout.addRow("账号", login)
        layout.addRow("口令", password)
        box = QDialogButtonBox(QDialogButtonBox.StandardButton.Ok | QDialogButtonBox.StandardButton.Cancel)
        box.button(QDialogButtonBox.StandardButton.Ok).setText("连接并读取列表")
        box.accepted.connect(dialog.accept)
        box.rejected.connect(dialog.reject)
        layout.addRow(box)
        try:
            if dialog.exec() != QDialog.DialogCode.Accepted:
                return
            session = open_connection(origin.text(), login.text(), password.text())
        except Exception:
            self.notice.setText("未连接。请核对 HTTPS 地址、账号和本机网络组件。")
            return
        finally:
            password.clear()
            dialog.deleteLater()
        self._owned = session
        self.ops = session.operations
        self._checking_connection = True
        self.notice.setText("正在连接并读取容器列表…")
        self._start(None, "读取列表", partial(session.establish, self._profile), self._entered)

    def _entered(self, established):
        self._owned, index = established
        self.ops = self._owned.operations
        self._populate(index)

    def original_checkout_changed(self):
        if self.editor is not None:
            local = self.editor.local
            if (local is not None and self._owned is local.active
                    and self.editor.host.archive.checkout.live is not local.owner.original):
                self._forget("原工作树会话已关闭，本机编辑接入已撤销。")
            else:
                self._paint()

    def _disconnect(self):
        self._forget("已断开连接。")

    def _forget(self, message):
        if self._owned is not None:
            self._owned.revoke()
        self._owned = self.ops = None
        self._profile = self._configured_profile
        self._checking_connection = self._empty_observed = False
        self._epoch += 1
        # Already-started requests can finish. Drop their late results here;
        # do not treat clearing the panel as canceling an original mutation.
        for task, _title, _done, _epoch in self.jobs.values():
            task.cancel()
        self.listed.clear()
        self.details.clear()
        self.outcomes.clear()
        self.recovery.clear()
        self.errors.clear()
        self._rebuild()
        self.notice.setText(message)

    def _current(self):
        item = self.entries.currentItem()
        return item.data(_ID) if item is not None else None

    def _available(self, key):
        return (self.ops is not None and key not in self.jobs
                and (self.editor is None or not self.editor.blocks(key))
                and (not self.jobs if key is None else None not in self.jobs))

    def _start(self, key, title, work, done):
        if not self._available(key):
            return
        if self.pool is None:
            self.pool = ThreadPoolExecutor(max_workers=_PARALLEL, thread_name_prefix="workspace-container")
        self.jobs[key] = (self.pool.submit(work), title, done, self._epoch)
        self.errors.pop(key, None)
        self.timer.start(125)
        self._retitle(key)
        self._paint()

    def _poll(self):
        for key, (job, _title, done, epoch) in tuple(self.jobs.items()):
            if not job.done():
                if epoch == self._epoch:
                    self._retitle(key)
                continue
            del self.jobs[key]
            if epoch != self._epoch:
                continue
            try:
                done(job.result())
            except PermissionError:
                self.details.pop(key, None)
                self.errors[key] = "当前连接没有这项权限。"
                if key is None:
                    self.details.clear()
                    self._populate(ContainerIndex(()), verified=False)
                elif key == self._current():
                    self._select()
            except Exception:
                if key is None:
                    self._empty_observed = False
                self.errors[key] = "未能核对本次请求。"
            self._retitle(key)
        if not self.jobs:
            self.timer.stop()
        if None in self.errors:
            if self._checking_connection:
                self._forget("未核对到已授权的容器列表，连接未接入。")
            else:
                self.notice.setText(self.errors[None])
        self._paint()

    def _load(self):
        self.notice.setText("正在读取容器列表…")
        self._start(None, "读取列表", partial(index_for, self.ops), self._populate)

    def _populate(self, index, *, verified=True):
        self._empty_observed = verified and not index.live
        self.listed = {container.id: container for container in index.live}
        local = {result.ticket.change.target.id: result for result in index.pending_recycles}
        for key in self.recovery.difference(local):
            if key not in self.listed:
                self.outcomes.pop(key, None)
        self.recovery = set(local)
        self.outcomes.update(local)
        self.details.clear()
        self._checking_connection = False
        self._rebuild()
        message = f"已读取 {len(index.live)} 个容器。" if index.live else "未看到可访问的容器。"
        if self._owned is not None:
            message += " 已接入本机写入配置。" if self._owned.writable else " 当前为只读连接。"
        if self.recovery:
            message += " 列表另显示本机原回收待查项。"
        if not index.local_complete:
            message += " 部分本机待查记录未能核对。"
        self.notice.setText(message)

    def _rebuild(self):
        prior = self._current()
        previous = self.entries.blockSignals(True)
        try:
            self.entries.clear()
            self.items.clear()
            rows = dict(self.listed)
            for key, result in self.outcomes.items():
                if unresolved(result) and (type(result.ticket.change) is not Recycle or key in self.recovery):
                    rows.setdefault(key, result.ticket.change.target)
            for row in rows.values():
                item = QListWidgetItem()
                item.setData(_ID, row.id)
                self.items[row.id] = (item, row)
                self.entries.addItem(item)
                self._retitle(row.id)
                if row.id == prior:
                    self.entries.setCurrentItem(item)
        finally:
            self.entries.blockSignals(previous)
        self._select()

    def _description(self, key):
        if key in self.jobs:
            work, title, _done, _epoch = self.jobs[key]
            return title + "…" if work.running() or work.done() else "排队：" + title
        if key in self.recovery:
            return "本机原回收待查"
        if key in self.errors:
            return self.errors[key]
        result = self.outcomes.get(key)
        if result is not None:
            return kind(result.ticket.change).value + "：" + result.phase.value
        return "未打开" if key not in self.details else "已读取"

    def _retitle(self, key):
        if key in self.items:
            item, row = self.items[key]
            item.setText(row.name + "  ·  " + self._description(key))

    def _open(self):
        key = self._current()
        if key in self.listed and self._available(key):
            self.details.pop(key, None)
            self._select()
            self._start(key, "打开容器", partial(open_checked, self.ops, key), self._opened)

    def _opened(self, info):
        key = info.container.id
        self.details[key] = info
        self.listed[key] = info.container
        if key in self.items:
            self.items[key] = (self.items[key][0], info.container)
        old = self.outcomes.get(key)
        if info.original is not None and (not unresolved(old) or old.ticket == info.original.ticket):
            self.outcomes[key] = info.original
        if key == self._current():
            self._select()

    def _select(self):
        info = self.details.get(self._current())
        previous = self.branches.blockSignals(True)
        try:
            self.branches.clear()
            if info is not None:
                for branch in sorted(info.branches, key=lambda b: (b.id != "main", b.id)):
                    self.branches.addItem(branch.id, branch)
        finally:
            self.branches.blockSignals(previous)
        self._pick_branch()

    def _pick_branch(self):
        branch = self.branches.currentData()
        previous = self.nodes.blockSignals(True)
        try:
            self.nodes.clear()
            if branch is not None:
                for node in sorted(branch.nodes, key=lambda n: n.id != branch.head):
                    suffix = "（分支头）" if node.id == branch.head else ""
                    self.nodes.addItem(node.id + suffix, node)
        finally:
            self.nodes.blockSignals(previous)
        self._paint()

    def _scope(self, action):
        key = self._current()
        info = self.details.get(key)
        if (info is None or key not in self.listed or not self._available(key)
                or (self.editor is not None and key == self.editor.uncertain_container)
                or unresolved(self.outcomes.get(key)) or action not in info.allowed
                or (info.container.revision == 0 and action is not Kind.SAVE)):
            return None
        return info

    def _choice(self):
        branch, node = self.branches.currentData(), self.nodes.currentData()
        return branch, node.id if node is not None else None

    def _paint(self):
        key = self._current()
        info = self.details.get(key)
        active = key is not None and self._available(key)
        branch, node = self.branches.currentData(), self.nodes.currentData()
        self.connect_button.setEnabled(self.ops is None and not self.jobs
                                       and (self.editor is None or not self.editor.busy()))
        self.disconnect_button.setEnabled(self.ops is not None)
        self.refresh.setEnabled(self._available(None))
        self.open_button.setEnabled(active and key in self.listed)
        self.rename_button.setEnabled(self._scope(Kind.RENAME) is not None)
        self.tags_button.setEnabled(self._scope(Kind.TAGS) is not None)
        self.recycle_button.setEnabled(self._scope(Kind.RECYCLE) is not None)
        mode = (save_mode(info, branch, node.id if node is not None else None)
                if self._scope(Kind.SAVE) is not None and branch is not None else None)
        self.save_button.setText("保存到新分支…" if mode == "fork" else "保存该容器的新节点…")
        self.save_button.setEnabled(mode is not None)
        can_branch = self._scope(Kind.CREATE_BRANCH)
        self.branch_button.setEnabled(can_branch is not None
                                      and branch_source(can_branch, branch, node.id if node is not None else None))
        result = self.outcomes.get(key)
        self.status_button.setEnabled(active and unresolved(result))
        if self.copy_transfer_button is not None:
            self.copy_transfer_button.setVisible(self.can_copy_transfer(result))
        retry_allowed = self._retry_selection() is not None
        self.retry_button.setVisible(retry_allowed)
        self.retry_button.setEnabled(retry_allowed)
        if self.ops is None:
            html = page("存储容器", "尚未连接 Workspace 存储客户端。")
        elif key is None:
            html = page("存储容器", "请选择容器，再打开信息。")
        else:
            item = self.items.get(key)
            title = item[1].name if item is not None else "存储容器"
            facts = [("容器标识", key)]
            if info is not None:
                facts.extend((("打开时的修订号", str(info.container.revision)),
                              ("标签", "、".join(info.tags) or "未设置")))
            if branch is not None:
                facts.append(("分支", branch.id))
                if node is None:
                    facts.append(("节点", "首个节点尚未保存"))
                else:
                    facts.extend((("节点", node.id), ("该节点正文", f"{node.length:,} 字节")))
            if result is not None:
                facts.extend((("本机提交号", result.ticket.submission), ("原操作号", result.ticket.operation)))
                if result.receipt is not None:
                    if result.receipt.revision is not None:
                        facts.append(("已确认修订", str(result.receipt.revision)))
                    if type(result.ticket.change) is CreateBranch:
                        facts.extend((("已新建分支", result.receipt.branch),
                                      ("继承原节点", result.receipt.node)))
                    elif result.receipt.node is not None:
                        facts.extend((("回执分支", result.receipt.branch),
                                      ("回执节点", result.receipt.node)))
            html = page(title, self._description(key), facts)
        if html != getattr(self, "_html", None):
            self._html = html
            self.browser.setHtml(html)
        if self.editor is not None:
            self.editor.paint()
        if self.cold is not None:
            self.cold.paint()

    def _submit(self, info, change):
        key = info.container.id
        if (key != self._current() or self._scope(kind(change)) is not info):
            return  # a modal dialog can outlive the selected native view
        self._start(key, kind(change).value, partial(perform, self.ops, change), self._observed)

    def _observed(self, result):
        key = result.ticket.change.target.id
        if key in self.recovery and (result.receipt is None or not result.receipt.recycled):
            self.notice.setText("尚未取得原回收回执；保留同一个待查操作。")
            return
        self.outcomes[key] = result
        change = result.ticket.change
        if result.receipt is not None and type(change) is Rename and key in self.listed:
            updated = replace(change.target, name=change.name, revision=result.receipt.revision)
            self.listed[key] = updated
            self.items[key] = (self.items[key][0], updated)
        if result.phase in (Phase.CONFIRMED, Phase.CONFLICT):
            self.details.pop(key, None)
            if key == self._current():
                self._select()
        if result.receipt is not None and result.receipt.recycled:
            self.recovery.discard(key)
            self.listed.pop(key, None)
            self._rebuild()
            cleanup = result.receipt.recycle_source_cleanup_pending
            suffix = ("；原盘清理待完成。" if cleanup is True else
                      "；原盘清理状态尚未核对。" if cleanup is None else "。")
            self.notice.setText("已确认原容器移入专用回收盘：" + key
                                + "；原操作 " + result.ticket.operation + suffix)

    def _rename(self):
        info = self._scope(Kind.RENAME)
        if info is None:
            return
        value, ok = QInputDialog.getText(self, "重命名容器", "新名称：", text=info.container.name)
        if ok and text_ok(value) and value != info.container.name:
            self._submit(info, Rename(info.container, value))

    def _tags(self):
        info = self._scope(Kind.TAGS)
        if info is None:
            return
        exact = any(any(c in tag for c in "\r\n\u2028\u2029\xa0") for tag in info.tags)
        original = json.dumps(info.tags, ensure_ascii=True) if exact else "\n".join(info.tags)
        hint = "标签 JSON 列表（整组替换）：" if exact else "每行一个标签（整组替换）："
        value, ok = QInputDialog.getMultiLineText(self, "替换标签", hint, original)
        if not ok:
            return
        try:
            tags = json.loads(value) if exact else [line for line in value.split("\n") if line]
        except ValueError:
            tags = None
        if type(tags) is not list or not tags_ok(tuple(tags)):
            self.notice.setText("没有替换标签：请核对文本格式和大小。")
        elif len(tags) != len(set(tags)):
            self.notice.setText("没有替换标签：有重复项。每个标签只保留一次后再试。")
        else:
            self._submit(info, Tags(info.container, tuple(tags)))

    def _recycle(self):
        info = self._scope(Kind.RECYCLE)
        if info is None:
            return
        target = info.container
        if confirm(self, "删除到专用回收盘", f"{target.name}\n容器 {target.id}\n修订 {target.revision}",
                   attestation="确认将这个原容器移入专用回收盘。", action_text="确认删除"):
            self._submit(info, Recycle(target))

    def _create_branch(self):
        info = self._scope(Kind.CREATE_BRANCH)
        source, node = self._choice()
        if info is None or not branch_source(info, source, node):
            return
        change = branch_choice(info, source, node, new_id())
        if change is None:
            self.notice.setText("新分支标识已被占用，本次未发起。")
            return
        target = change.target
        if (confirm(self, "从所选原节点新建分支",
                    f"{target.name!r} · 容器 {target.id}\n"
                    f"来源分支 {change.source_branch} · 原节点 {change.source_node}\n"
                    f"新分支 {change.branch}",
                    attestation="只新建分支指向这个节点，不上传内容或改变原分支。",
                    action_text="确认新建分支")
                and self._choice() == (source, node)):
            self._submit(info, change)

    def _save(self):
        info = self._scope(Kind.SAVE)
        branch, node = self._choice()
        if info is None or branch is None:
            return
        mode = save_mode(info, branch, node)
        change = save_choice(info, branch, node)
        if change is None:
            return
        target, body = change.target, change.body
        base = body.selected_base if body.selected_base else "首次 main"
        destination = ("从该历史节点新建分支并在同一次保存产生新节点；main 不变。"
                       if mode == "fork" else f"保存到分支 {change.branch}。")
        if confirm(self, "保存所选容器", f"{target.name}\n容器 {target.id}\n"
                   f"原节点 {base}\n封存正文 {body.length:,} 字节\n{destination}",
                   attestation="确认提交这个容器已封存的完整正文。", action_text="确认保存"):
            if self._choice() == (branch, node):
                self._submit(info, change)

    def can_copy_transfer(self, result):
        return (self.cold is not None and self._profile is not None and self._owned is not None
                and self._owned.writable and self.ops is self._owned.operations and unresolved(result)
                and self._available(result.ticket.change.target.id))

    def copy_transfer(self, result):
        if not self.can_copy_transfer(result):
            return
        from .container_cold_controls import copy_reference
        if copy_reference(self._profile._state_selection[0], result.ticket):
            self.notice.setText("已复制原 STATE 和三个原号。不包含凭据。")

    def _copy_selected_transfer(self):
        self.copy_transfer(self.outcomes.get(self._current()))

    def _retry_selection(self):
        key, own = self._current(), self._owned
        result = self.outcomes.get(key)
        if (own is None or not own.writable or self.ops is not own.operations or not self._available(key)
                or key not in self.listed or key in self.recovery or result is None
                or result.phase is not Phase.ABSENT or result.ticket.change.target.id != key
                or self._profile is None or kind(result.ticket.change) not in self._profile._allowed
                or (self.editor is not None and key == self.editor.uncertain_container)):
            return None
        return own, self._epoch, result

    def _retry(self):
        selected = self._retry_selection()
        if selected is None:
            return
        own, epoch, prior = selected
        ticket = prior.ticket
        target = ticket.change.target
        if not confirm(self, "重试这笔原操作",
                       f"{kind(ticket.change).value} · {target.name!r}\n"
                       f"容器 {target.id}\n原操作 {ticket.operation}",
                       attestation="按原号再次核对；服务器仍明确没有回执时才发送同一笔。",
                       action_text="核对并重试原操作"):
            return
        current = self._retry_selection()
        if current is not None and current[0] is own and current[1] == epoch and current[2] is prior:
            self._start(target.id, "核对并重试原操作",
                        partial(retry, own.operations, prior, confirmed=True), self._observed)

    def _status(self):
        key = self._current()
        result = self.outcomes.get(key)
        if unresolved(result):
            self._start(key, "查询原回执", partial(lookup, self.ops, result.ticket), self._observed)
