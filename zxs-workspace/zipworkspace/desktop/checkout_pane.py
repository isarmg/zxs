"""Explicitly launch a new CLI-format working tree; never open or discover previous state."""
from PySide6.QtWidgets import QFileDialog, QInputDialog, QVBoxLayout, QWidget

from .checkout_action import CheckoutChoice, available
from .widgets import button, confirm, label


class CheckoutPane(QWidget):
    def __init__(self, owner, current_store):
        super().__init__()
        self.owner, self.current_store = owner, current_store
        self.snapshot, self.active, self.unresolved = None, False, False
        layout = QVBoxLayout(self)
        self.selected = label()
        layout.addWidget(self.selected)
        layout.addWidget(label("从当前显示的原节点创建一份新工作树。此入口采用命令行的 STATE："
                               "你选择本人独占的状态父目录，不会登记为可自动打开的桌面托管实例。"))
        self.start = button("签出此快照到新目录…", self.pick)
        layout.addWidget(self.start)
        self.result = label()
        layout.addWidget(self.result)
        layout.addStretch()
        self.set_snapshot(None)

    def set_snapshot(self, selected):
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
            plan = CheckoutChoice(snapshot)
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
        destination, marker = result
        self.result.setText(f"已按原操作签出：{destination}\n原节点：{marker.operation}\n"
                            f"命令行 STATE：{plan.state}\n签出操作：{plan.operation}")
        self.owner.inform("新的 CLI 工作树已创建；不会因为切换页面或关闭窗口而再次签出。")
        self._release()
