"""Explicitly launch a new CLI-format working tree; never open or discover previous state."""
from contextlib import closing
from PySide6.QtWidgets import QDialog, QDialogButtonBox, QFileDialog, QHBoxLayout, QInputDialog, QVBoxLayout, QWidget

from ..lifecycle import observe_live_changes
from ..model import Rejected

from .checkout_action import CheckoutChoice, available
from .commit_controls import CommitControls
from .widgets import Table, button, confirm, label


class CheckoutPane(QWidget):
    def __init__(self, owner, current_store):
        super().__init__()
        self.owner, self.current_store = owner, current_store
        self.snapshot, self.active, self.unresolved = None, False, False
        self.live = self.changes_dialog = None
        layout = QVBoxLayout(self)
        self.selected = label()
        layout.addWidget(self.selected)
        layout.addWidget(label("从当前显示的原节点创建一份新工作树。此入口采用命令行的 STATE："
                               "你选择本人独占的状态父目录，不会登记为可自动打开的桌面托管实例。"))
        self.start = button("签出此快照到新目录…", self.pick)
        layout.addWidget(self.start)
        self.result = label()
        layout.addWidget(self.result)
        self.refresh = button("刷新本窗口工作树的状态", self.refresh_live)
        layout.addWidget(self.refresh)
        self.changes = button("查看工作目录相对基线的改动…", self.compare_live)
        layout.addWidget(self.changes)
        self.details = label()
        layout.addWidget(self.details)
        self.commits = CommitControls(owner, self.refresh_live)
        layout.addWidget(self.commits)
        self.close_session()
        layout.addStretch()
        self.set_snapshot(None)

    def set_snapshot(self, selected):
        if self.snapshot is not selected and self.live is not None: self.close_session()
        self.snapshot = selected
        if not available(): message = "当前平台未开放此写入口。可继续浏览或核验快照。"
        elif selected is None: message = "先打开一个 .zxs，或从历史 / 分支页面打开要签出的确切节点。"
        else: message = f"当前快照：{selected.path}\n要签出的原节点：{selected.browser.commit.operation}"
        self.selected.setText(message)
        self.start.setEnabled(selected is not None and not self.active and not self.unresolved and available())

    def _release(self):
        self.active = False
        self.set_snapshot(self.snapshot)

    def pick(self):
        snapshot = self.snapshot
        if snapshot is None or self.active or self.unresolved or self.owner.pending is not None or not available(): return
        self.active = True
        self.start.setEnabled(False)
        plan = None
        queued = False
        def directory(prompt, initial=""):
            opts = QFileDialog.Option.ShowDirsOnly | QFileDialog.Option.DontResolveSymlinks
            return QFileDialog.getExistingDirectory(self, prompt, initial, opts)
        try:
            plan = CheckoutChoice(snapshot, keep_session=True)
            previous = self.current_store()
            name = directory("为这份新工作树选择归档存储目录", str(previous.path) if previous else "")
            if not name: return
            plan.pick_store(name)
            if previous is not None and plan.store.path == previous.path:
                previous.check()  # Choosing the known name must not silently adopt a replacement store.
            name = directory("选择新工作目录的父目录")
            if not name: return
            plan.pick_target(name)
            name = directory("选择已有的专用 CLI 状态父目录（本人独占、无同步）")
            if not name: return
            plan.pick_private(name)
            leaf, ok = QInputDialog.getText(self, "新目录名", "输入工作目录和独立 STATE 各自使用的同一个新名称：")
            if not ok: return
            def reviewed(details):
                try:
                    if self.snapshot is not snapshot or not confirm(self, "确认本次签出", details,
                            attestation="我确认所选 CLI 状态父目录由本人独占，操作及恢复期间不会被同步、移动或由其他程序修改。"):
                        self._discard(plan, "未确认本次签出，没有写入。")
                        return
                    if self.snapshot is not snapshot:
                        self._discard(plan, "当前节点已经改变，未开始签出。")
                        return
                    self.result.setText(f"本次操作：{plan.operation}\nSTATE：{plan.state}\n正在创建；请勿重复提交。")
                    self._submit(plan, "正在按原节点创建新的 CLI 工作树…", plan.run,
                                 lambda result: self._complete(plan, result))
                except Exception as err:
                    self._discard(plan, str(err))
            self._submit(plan, "正在核验三处目录及新名称（只读）…", lambda: plan.review(leaf), reviewed)
            queued = True
        except Exception as err:
            self.result.setText(str(err))
        finally:
            if not queued:
                if plan is not None: plan.close()
                self._release()

    def _discard(self, plan, message):
        if plan.used and plan.publication is not None:
            self.unresolved = True
            message += "\n本窗口已停止发起新签出，避免对不明结果重复提交。"
        plan.close()
        self.result.setText(message)
        self.owner.inform(message)
        self._release()

    def _submit(self, plan, title, work, accept):
        def failed(err):
            self._discard(plan, str(err) or "任务未完成；未自动重试。")
        future = self.owner.job(title, work, accept, failed=failed)
        if future is None:
            self._discard(plan, "另一个操作正在执行；本步骤未开始。")
            return
        # A queued job cancelled before start never enters its function.
        future.add_done_callback(lambda f: plan.close() if f.cancelled() else None)

    def _complete(self, plan, result):
        # The checkout is already durable. A late or failed view may display the receipt,
        # but must never leak its handles or adopt it as the currently selected worktree.
        incoming = plan.take_session()
        self.active = False
        try:
            self.set_snapshot(self.snapshot)
            destination, marker = result
            self.result.setText(f"原操作已签出：{destination}\n原节点：{marker.operation}\n"
                                f"命令行 STATE：{plan.state}\n签出操作：{plan.operation}")
            if self.snapshot is not plan.snapshot:
                self.owner.inform("原签出已完成；当前快照已变，未交接工作树会话。")
                return
            self.close_session()
            self.owner.inform("新的 CLI 工作树已创建；不会因为切换页面或关闭窗口而再次签出。")
            self.refresh.setEnabled(incoming is not None)
            self.changes.setEnabled(incoming is not None)
            self.commits.set_session(incoming)
            self.live, incoming = incoming, None
        finally:
            if incoming is not None: incoming.close()
            plan.close()
        if self.live is not None: self.refresh_live()

    def close_session(self):
        if self.changes_dialog is not None: self.changes_dialog.reject()
        self.commits.set_session(None)
        if self.live is not None: self.live.close()
        self.live = None
        self.refresh.setEnabled(False)
        self.changes.setEnabled(False)
        self.details.setText("本窗口未持有当前工作树；不会按路径搜索或接管已有 STATE。")

    def compare_live(self):
        live = self.live
        if (live is None or self.owner.pending is not None or self.active
                or self.commits.running is not None or self.changes_dialog is not None): return
        def done(outcome):
            if self.live is not live: return
            result, error = outcome
            if error is not None:
                self.owner.inform(f"未取得完整工作树对照：{error}。未启动冻结、提交或恢复。")
                return
            self.owner.inform("工作树只读对照已完成；后续编辑不会自动刷新。")
            self.show_changes(live, *result)

        loan = None
        try:
            loan = live.borrow()  # Only dup here; filesystem and catalog qualification are on the worker.
            def read():
                # Route a late refusal through the same session check as a late success.
                try:
                    with closing(loan): return observe_live_changes(loan), None
                except Exception as error: return None, error
            future = self.owner.job("正在只读核验实际基线及本次工作目录…", read, done)
            if future is None: raise Rejected("另一个任务正在执行；未开始对照")
        except BaseException as error:
            if loan is not None: loan.close()
            if not isinstance(error, Exception): raise
            done((None, error))
        else:
            future.add_done_callback(lambda task: loan.close() if task.cancelled() else None)

    def show_changes(self, live, marker, report):
        # The read-only result exists only in this modal view. Once closed it
        # cannot linger into a later commit, recovery, or baseline switch.
        dialog = QDialog(self)
        dialog.setWindowTitle("工作目录相对基线的改动")
        dialog.resize(820, 510)
        layout = QVBoxLayout(dialog)
        layout.addWidget(label(
            f"原工作目录：{live.work}\n当前登记基线：分支 {marker.branch} · 第 {marker.generation} 代\n"
            f"基线节点：{marker.operation}\n新增 {report.added} · 修改 {report.changed} · 缺少 {report.missing}。"
            + ("本次未发现差异。" if not report.rows else "")))
        layout.addWidget(label("这是一次观察，不是实时或原子快照，也不能代替提交前停止写入者。"))
        table = Table(("相对路径", "差异", "登记基线", "工作目录"), "本工作树的只读改动")
        layout.addWidget(table)
        nav = QHBoxLayout(); layout.addLayout(nav)
        number, count = 0, max(1, (len(report.rows) + report.PAGE - 1) // report.PAGE)
        change = {"added":"新增", "changed":"修改", "missing":"缺少"}
        def side(kind, size):
            return f"文件 · {size:,} B" if kind == "file" else "目录" if kind == "directory" else "无"
        def page(i):
            nonlocal number
            table.rows.replace(((r.path, change[r.change], side(r.before, r.before_bytes),
                                 side(r.after, r.after_bytes)), r) for r in report.page(i))
            number = i
            at.setText(f"第 {i+1}/{count} 页")
            back.setEnabled(i > 0); next_.setEnabled(i+1 < count)
        back = button("上一页", lambda: page(number-1))
        next_ = button("下一页", lambda: page(number+1))
        at = label()
        for item in (back, next_, at): nav.addWidget(item)
        controls = QDialogButtonBox(QDialogButtonBox.StandardButton.Close)
        controls.rejected.connect(dialog.reject)
        layout.addWidget(controls)
        page(0)
        def dismissed(_):
            self.changes_dialog = None
            dialog.deleteLater()
        dialog.finished.connect(dismissed)
        self.changes_dialog = dialog
        dialog.open()  # Window-modal, no nested event loop.

    def refresh_live(self):
        live = self.live
        if live is None or self.owner.pending is not None or self.active: return
        def shown(record):
            if self.live is not live: return
            phase = {"visible":"可见", "hidden":"已隐藏", "hide-pending":"隐藏待决（位置未确认）",
                     "restore-pending":"恢复待决（位置未确认）"}[record["visibility"]]
            self.details.setText(
                f"本次签出的原工作树：{live.work}\n命令行 STATE：{live.state}\n归档存储：{live.store}\n"
                f"实例：{record['worktree']}\n可见状态：{phase}\n分支：{record['branch']} · 基线第 {record['base_generation']} 代\n"
                f"基线节点：{record['base_operation']}\n待决提交：{record['pending_operation'] or '无'}\n"
                f"待决隐藏/恢复：{record['pending_visibility_operation'] or '无'}\n"
                "这是原登记的一次只读快照，未检查工作文件是否已修改。")
            self.owner.inform("已读取本窗口原工作树状态。")
        def failed(error):
            if self.live is live: self.details.setText(f"无法核证原工作树状态：{error}。不会认领替件。")
        try:
            self.owner.job("正在读取本次工作树状态…", live.status, shown, failed=failed)
        except Exception as error:
            failed(error)
