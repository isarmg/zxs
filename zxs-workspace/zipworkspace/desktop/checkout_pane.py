"""Explicitly launch a new CLI-format working tree; never open or discover previous state."""
from contextlib import closing
from PySide6.QtWidgets import QDialog, QDialogButtonBox, QFileDialog, QHBoxLayout, QInputDialog, QVBoxLayout, QWidget

from ..lifecycle import observe_live_changes
from ..model import Rejected

from .checkout_action import CheckoutChoice, available
from .commit_controls import CommitControls
from .initialize_choice import InitializeChoice, available as can_initialize
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
        layout.addWidget(label("签出快照到新目录，或从已有工作目录新建仓库。两者都采用命令行的 STATE："
                               "你选择本人独占的状态父目录，不会登记为可自动打开的桌面托管实例。"))
        self.start = button("签出此快照到新目录…", self.pick)
        self.initialize = button("从已有工作目录新建仓库…", self.pick_existing)
        layout.addWidget(self.start)
        layout.addWidget(self.initialize)
        self.result = label()
        layout.addWidget(self.result)
        self.session_label = label()
        layout.addWidget(self.session_label)
        self.end = button("关闭本窗口的工作树会话…", self.end_session)
        layout.addWidget(self.end)
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

    def set_snapshot(self, selected):
        live = self.live
        if live is not None and self.snapshot is not selected:
            live.revoke_branch_source()  # Does not revoke the worktree or any recorded operation.
        self.snapshot = selected
        supported, new_supported = available(), can_initialize()
        if not supported and not new_supported: message = "当前平台未开放此写入口。可继续浏览或核验快照。"
        elif not supported: message = "当前不能从快照签出；可从已有工作目录登记新仓库。"
        elif selected is None: message = "浏览区没有快照；可从 .zxs、历史或分支中选择版本。"
        else: message = f"浏览／下次签出的候选：{selected.path}\n显示节点：{selected.browser.commit.operation}"
        if live is not None: message += "\n已持有工作树；先关闭该会话才能创建另一个。"
        self.selected.setText(message)
        vacant = live is None and not self.active and not self.unresolved
        self.start.setEnabled(supported and selected is not None and vacant)
        self.initialize.setEnabled(new_supported and vacant)
        if live is None:
            caption = "本窗口未持有工作树，不会按路径接管已有 STATE。"
        else:
            caption = (f"本窗口操作绑定的原工作目录：{live.work}\n原 STATE：{live.state}\n原 STORE：{live.store}\n"
                       "切换浏览不改变本会话；关闭会话或退出窗口后，不能在窗口凭路径重新打开。")
            if live.checkout_marker is None:
                caption += "\n本会话从已有目录创建，不关联任何浏览中的快照。"
            elif not live.branch_source_available:
                caption += "\n已离开签出时的原快照；此会话不能再从它新建分支。"
        self.session_label.setText(caption)
        self.commits.buttons()

    def _release(self):
        self.active = False
        self.set_snapshot(self.snapshot)

    def _directory(self, prompt, initial="", *, read_only=False):
        opts = QFileDialog.Option.ShowDirsOnly | QFileDialog.Option.DontResolveSymlinks
        if read_only: opts |= QFileDialog.Option.DontUseNativeDialog | QFileDialog.Option.ReadOnly
        return QFileDialog.getExistingDirectory(self, prompt, initial, opts)

    def pick_existing(self):
        if (not can_initialize() or self.live is not None or self.active or self.unresolved
                or self.owner.pending is not None): return
        self.active = True
        self.set_snapshot(self.snapshot)
        plan = None
        queued = False

        def current():
            return self.active and self.live is None and not self.unresolved

        try:
            plan = InitializeChoice()
            for prompt, pick in (("选择要登记的现有工作目录", plan.pick_work),
                                 ("选择保存后续快照的已有归档存储目录", plan.pick_store),
                                 ("选择已有的专用 CLI 状态父目录（本人独占、无同步）", plan.pick_private)):
                path = self._directory(prompt, read_only=True)
                if not path: return
                pick(path)
            leaf, ok = QInputDialog.getText(self, "新 STATE 名称", "输入在专用私有父目录下要创建的一个名称：")
            if not ok: return

            def reviewed(details):
                try:
                    if not current() or not confirm(
                            self, "新建仓库，只登记工作目录", details,
                            attestation="我确认这三处目录在操作期间不被移动；专用 STATE 父由本人独占且无同步。",
                            action_text="仅新建登记"):
                        self._discard(plan, "未确认创建，没有写入新 STATE。")
                        return
                    if not current():
                        self._discard(plan, "窗口中的工作树会话已变，没有创建新 STATE。")
                        return
                    self._submit(plan, "正在独占创建并同步本次 CLI STATE；未提交快照…",
                                 lambda: plan.run(confirmed=True), lambda _: self.initialized(plan, current()))
                except Exception as err:
                    self._discard(plan, str(err))

            self._submit(plan, "正在只读核验三个原目录及新 STATE 名称…", lambda: plan.review(leaf), reviewed)
            queued = True
        except Exception as err:
            self.result.setText(str(err))
        finally:
            if not queued:
                if plan is not None: plan.close()
                self._release()

    def initialized(self, plan, current):
        incoming = plan.take_session()
        try:
            self.result.setText(f"已核验新 CLI 登记：{plan.state}\n新实例：{plan.instance}\n尚无已提交快照。")
            if current and incoming is not None:
                self._attach(incoming)
                incoming = None
                self.owner.inform("已登记原工作目录。要生成第 1 代，请停止写入后另行确认提交。")
            else:
                self.owner.inform("原登记已完成，但没有接管到当前窗口。请保留原 STATE。")
        finally:
            if incoming is not None: incoming.close()
            plan.close()
            self._release()
        if self.live is not None: self.refresh_live()

    def pick(self):
        snapshot = self.snapshot
        if (snapshot is None or self.live is not None or self.active or self.unresolved
                or self.owner.pending is not None or not available()): return
        self.active = True
        self.set_snapshot(self.snapshot)
        plan = None
        queued = False
        try:
            plan = CheckoutChoice(snapshot, keep_session=True)
            previous = self.current_store()
            name = self._directory("为这份新工作树选择归档存储目录", str(previous.path) if previous else "")
            if not name: return
            plan.pick_store(name)
            if previous is not None and plan.store.path == previous.path:
                previous.check()  # Choosing the known name must not silently adopt a replacement store.
            name = self._directory("选择新工作目录的父目录")
            if not name: return
            plan.pick_target(name)
            name = self._directory("选择已有的专用 CLI 状态父目录（本人独占、无同步）")
            if not name: return
            plan.pick_private(name)
            leaf, ok = QInputDialog.getText(self, "新目录名", "输入工作目录和独立 STATE 各自使用的同一个新名称：")
            if not ok: return
            def reviewed(details):
                try:
                    if self.snapshot is not snapshot or self.live is not None or not confirm(self, "确认本次签出", details,
                            attestation="我确认所选 CLI 状态父目录由本人独占，操作及恢复期间不会被同步、移动或由其他程序修改。"):
                        self._discard(plan, "未确认本次签出，没有写入。")
                        return
                    if self.snapshot is not snapshot or self.live is not None:
                        self._discard(plan, "当前节点或工作树会话已经改变，未开始签出。")
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
            message += "\n本窗口已停止创建或签出工作树，避免对不明结果重复发起。"
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

    def _attach(self, original):
        self.end.setEnabled(original is not None)
        self.refresh.setEnabled(original is not None)
        self.changes.setEnabled(original is not None and original.initial_base is None)
        self.commits.set_session(original)
        self.live = original

    def _complete(self, plan, result):
        # The checkout is already durable. A late or failed view may display the receipt,
        # but must never leak its handles or adopt it as the currently selected worktree.
        incoming = plan.take_session()
        self.active = False
        try:
            destination, marker = result
            self.result.setText(f"原操作已签出：{destination}\n原节点：{marker.operation}\n"
                                f"命令行 STATE：{plan.state}\n签出操作：{plan.operation}")
            if self.snapshot is not plan.snapshot or self.live is not None:
                self.owner.inform("原签出已完成；浏览或会话已变，未接管这份工作树。")
                return
            self.owner.inform("新的 CLI 工作树已创建。" if incoming is not None else
                              "CLI 签出已完成；本窗口未获得会话凭据，请保留原 STATE。")
            self._attach(incoming)
            incoming = None
        finally:
            if incoming is not None: incoming.close()
            plan.close()
            self.set_snapshot(self.snapshot)
        if self.live is not None: self.refresh_live()

    def end_session(self):
        live, last = self.live, self.commits.last
        def idle():
            return (self.owner.pending is None and not self.active and self.commits.running is None
                    and self.changes_dialog is None)
        if live is None or not idle(): return
        origin = (f"签出操作：{live.checkout_operation}" if live.checkout_operation is not None else
                  f"原登记实例：{live.instance}")
        details = f"原工作目录：{live.work}\n原 STATE：{live.state}\n原 STORE：{live.store}\n{origin}\n"
        if last is not None:
            details += f"本窗口最近的操作（不代表已受理或已完成）：\n{self.commits.operation_numbers(last)}\n"
        query = "status 或 checkout-status" if live.checkout_operation is not None else "status"
        details += ("\n只关闭本窗口的会话，不删除、隐藏、提交或恢复任何内容，也不取消已有待决。"
                    f"需要继续时，请先用命令行 {query} 核对原 STATE；窗口不会重新接管。")
        if not confirm(self, "关闭本窗口的工作树会话", details,
                       attestation="我已记录需要保留的原 STATE 和操作号。",
                       action_text="仅关闭窗口会话"):
            return
        if self.live is not live or self.commits.last is not last or not idle():
            self.owner.inform("会话或操作已改变；没有关闭工作树会话。")
            return
        self.close_session()
        self.owner.inform(f"只关闭了本窗口的工作树会话；原文件和 STATE 未改动。\n原 STATE：{live.state}")

    def close_session(self):
        if self.changes_dialog is not None: self.changes_dialog.reject()
        live, self.live = self.live, None
        if live is not None: live.revoke_branch_source()
        try:
            self.commits.set_session(None)
        finally:
            if live is not None: live.close()
            self.end.setEnabled(False)
            self.refresh.setEnabled(False)
            self.changes.setEnabled(False)
            self.details.setText("")
            self.set_snapshot(self.snapshot)

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
            gen = record["base_generation"]
            base = (f"基线第 {gen} 代\n基线节点：{record['base_operation']}" if gen else
                    "尚无已提交快照")
            self.changes.setEnabled(bool(gen))
            self.details.setText(
                f"实例：{record['worktree']}\n可见状态：{phase}\n分支：{record['branch']} · {base}\n"
                f"待决提交：{record['pending_operation'] or '无'}\n"
                f"待决隐藏/恢复：{record['pending_visibility_operation'] or '无'}\n"
                "这是原登记的一次只读快照，未检查工作文件是否已修改。")
            self.owner.inform("已读取本窗口原工作树状态。")
        def failed(error):
            if self.live is live: self.details.setText(f"无法核证原工作树状态：{error}。不会认领替件。")
        try:
            self.owner.job("正在读取本次工作树状态…", live.status, shown, failed=failed)
        except Exception as error:
            failed(error)
