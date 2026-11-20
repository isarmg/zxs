"""Separate live-tree commit, selected-node branch, and original-operation recovery decisions."""
from PySide6.QtWidgets import QHBoxLayout, QVBoxLayout, QWidget

from ..capacity import describe
from .branch_choice import SnapshotBranch
from .commit_choice import CommitChoice
from .pending_fork import PendingFork
from .retry_choice import PendingRetry
from .widgets import button, confirm, label


class CommitControls(QWidget):
    def __init__(self, owner, refresh):
        super().__init__()
        self.owner, self.refresh_status = owner, refresh
        self.session = self.last = self.running = None
        self.blocked = self.visible = False
        box = QVBoxLayout(self); row = QHBoxLayout()
        self.save = button("提交本次工作目录…", self.begin)
        self.new_branch = button("从签出时的原节点创建新分支…", self.branch)
        self.query = button("查询该次操作的原回执", self.inspect)
        self.sync_receipt = button("只同步本机原完成回执…", self.confirm_completion)
        self.retry = button("核对并续作原待决操作…", self.resume)
        self.conflict_fork = button("核对同代冲突，用原冻结归档另建分支…", self.fork_conflict)
        row.addWidget(self.save); row.addWidget(self.query); box.addLayout(row)
        box.addWidget(self.new_branch); box.addWidget(self.retry); box.addWidget(self.conflict_fork)
        box.addWidget(self.sync_receipt)
        self.output = label(); box.addWidget(self.output)
        self.set_session(None)

    def buttons(self):
        idle = self.session is not None and self.running is None and not self.blocked and self.visible
        self.save.setEnabled(idle)
        self.new_branch.setEnabled(idle and self.session.branch_source_available)
        free = self.session is not None and self.running is None and self.last is not None
        self.query.setEnabled(free)
        self.sync_receipt.setEnabled(free and self.last.local_receipt_seen)
        self.retry.setEnabled(free and self.visible and self.last.pending_seen and not self.last.conflict)
        self.conflict_fork.setEnabled(free and self.visible and self.last.conflict_pending_seen)

    def allow_visible(self, visible):
        self.visible = visible
        self.buttons()

    @staticmethod
    def operation_numbers(job):
        if isinstance(job, PendingFork):
            return f"原冲突操作：{job.old.operation}\n指定后继：{job.operation}"
        return f"操作号：{job.operation}"

    def set_session(self, live):
        if self.last is not None and self.last is not self.running: self.last.release()
        self.session, self.last, self.blocked = live, None, False
        self.visible = live is not None
        self.output.setText("只操作本窗口刚创建的原工作树，不从 STATE 路径补领。")
        self.buttons()

    def discard(self, job):
        if self.last is job: self.last = None
        try: job.release()
        except (OSError, ValueError) as error: self.owner.inform(describe(error))
        finally:
            if self.running is job: self.running = None
            self.buttons()

    def submit(self, job, title, run, apply, failed):
        try:
            future = self.owner.job(title, run, apply, failed=failed)
        except Exception as exc:
            failed(exc); return
        if future is None:
            failed(RuntimeError("另一个任务正在执行；本步骤未启动。"))

    def begin(self):
        self._begin(branch=False)

    def branch(self):
        self._begin(branch=True)

    def _begin(self, *, branch):
        original = self.session
        if (original is None or self.running is not None or self.blocked or not self.visible
                or self.owner.pending is not None): return
        def same_decision():
            return self.session is original and (not branch or original.branch_source_available)
        if not same_decision(): return
        try: job = SnapshotBranch(original) if branch else CommitChoice(original)
        except Exception as err:
            self.output.setText(describe(err)); return
        self.running = job; self.buttons()

        def refused(err):
            if self.session is original: self.output.setText(describe(err))
            self.discard(job)

        def after_submit(message, *, uncertain):
            if self.session is not original:
                self.discard(job)
                self.owner.inform(f"原工作树提交操作 {job.operation} 已返回；未把它用于新的选择。")
                return
            self.last, self.running, self.blocked = job, None, True
            self.buttons()  # Keep query available even if rendering the durable reply fails.
            self.output.setText(f"{message}\n原 STATE：{job.live.state}\n操作号：{job.operation}")
            self.blocked = uncertain; self.buttons()
            if not uncertain: self.refresh_status()

        def reviewed(detail):
            try:
                if branch:
                    question, action = "从签出时的原节点创建并发布新分支", "创建、发布并切换"
                    agree = "我确认发布的是所列原快照，不含当前编辑。STATE 由本人独占；STORE 不同步，无其他程序改动本次暂存名。"
                else:
                    question, action = "确认提交整个工作目录", "确认并提交"
                    agree = "我已停止所有工作树写入者。STATE 由本人独占，STORE 不同步、不会有其他程序改动本次上传暂存名。"
                if not same_decision():
                    refused("原工作树会话或原分支来源已变；没有制作候选。"); return
                if not confirm(self, question, detail, attestation=agree, action_text=action):
                    refused("未确认，没有制作候选或发布。"); return
                if not same_decision():
                    refused("确认期间原工作树会话或原分支来源已变；没有制作候选。"); return
                if self.last is not None:
                    self.last.release(); self.last = None
                self.output.setText(f"已确认原操作 {job.operation}；不要重复提交。")
                def failed(error):
                    if not job.used:
                        refused(error); return
                    after_submit(f"{describe(error)}\n尚不能从此次返回确定结果；只读查询本次原回执。", uncertain=True)
                def done(result):
                    path, marker = result
                    saved = "新分支已发布，原操作回执已完成（当前基线以刷新状态为准）" if branch else "新快照已存入"
                    after_submit(f"{saved}：{path}\n分支：{marker.branch} · 第 {marker.generation} 代\n新节点：{marker.operation}",
                                 uncertain=False)
                title = "正在从所选原节点创建并发布新分支…" if branch else "正在冻结、核对并发布本次工作树…"
                self.submit(job, title, job.run, done, failed)
            except Exception as err:
                if job.used: after_submit(f"{describe(err)}\n结果待核。", uncertain=True)
                else: refused(err)
        self.submit(job, "正在只读核对原登记及 STORE 基线…", job.prepare, reviewed, refused)

    def inspect(self):
        job, original = self.last, self.session
        if job is None or original is None or self.running is not None or self.owner.pending is not None: return
        self.running = job; self.buttons()
        def end(kind, text):
            if self.session is not original:
                self.discard(job); return
            self.running = None
            self.blocked = kind != "completed"
            self.buttons()
            self.output.setText(f"{text}\nSTATE：{job.live.state}\n{self.operation_numbers(job)}")
        self.submit(job, "正在只读查询本次原操作…", job.result,
                    lambda result: end(*result), lambda error: end("unknown", f"原结果未知：{describe(error)}。未发布或重试。"))

    def confirm_completion(self):
        job, original = self.last, self.session
        if (job is None or original is None or self.running is not None or self.owner.pending is not None
                or not job.local_receipt_seen): return
        self.running = job; self.buttons()

        def current():
            return self.session is original and self.last is job and self.running is job

        def finished(text, *, complete=False):
            if not current(): self.discard(job); return
            self.running = None; self.blocked = not complete
            self.output.setText(f"{text}\n原 STATE：{job.live.state}\n{self.operation_numbers(job)}")
            self.buttons()
            if complete: self.refresh_status()

        try:
            detail = (f"原 STATE：{job.live.state}\n{self.operation_numbers(job)}\n\n"
                      "只复核 SQLite 已返回的同号完成行并同步本机原目录。"
                      "不读取、上传或确认 STORE 文件，不改工作文件、分支或历史。")
            if not confirm(self, "只补本机成功回执的目录同步", detail,
                           attestation="我确认原 STATE 仍是本会话独占，没有并行 CLI 修改。",
                           action_text="只同步本机回执"):
                finished("未确认，没有同步或重发。"); return
            if not current():
                finished("原会话或操作已变，没有同步。"); return
            self.submit(job, "正在核验原本机回执并同步原 STATE…",
                        lambda: job.confirm_local_receipt(confirmed=True),
                        lambda text: finished(text, complete=True),
                        lambda err: finished(describe(err) + "\n不重发；先重新只读核对原回执。"))
        except Exception as err:
            finished(describe(err))

    def resume(self):
        original, live = self.last, self.session
        if (not original or live is None or self.running is not None or self.owner.pending is not None
                or not self.visible or not original.pending_seen or original.conflict): return
        try: attempt = PendingRetry(original)
        except Exception as err:
            self.output.setText(describe(err)); self.buttons(); return
        self.running = attempt; self.buttons()

        def finish_view(message, *, completed=False):
            current = self.session is live and self.last is original
            try:
                if current:
                    self.blocked = not completed
                    self.output.setText(f"{message}\n原 STATE：{attempt.live.state}\n原操作：{attempt.operation}")
                else:
                    self.owner.inform("原操作核对已返回；当前工作树已切换，未把回执用于新的选择。")
            finally: self.discard(attempt)
            if current and completed: self.refresh_status()

        def reviewed(details):
            try:
                if self.session is not live or self.last is not original:
                    finish_view("工作树已切换，未提交续作。"); return
                terms = "原 STATE 仍由本人独占，STORE 不同步、无人改动本次暂存名。我确认只处理所列原操作。"
                if original.purpose == "branch": terms += " 我同意登记完成后把这个工作树切到所列目标分支。"
                if not confirm(self, "只处理原冻结操作", details,
                        attestation=terms, action_text=attempt.confirmation_label):
                    finish_view("未确认，未续作。再次尝试须先查询原回执。"); return
                if self.session is not live or self.last is not original:
                    finish_view("工作树已切换，未提交续作。"); return
                self.submit(attempt, "正在处理这笔原冻结操作…", attempt.run,
                            lambda outcome: finish_view(f"原操作回执已完成：{outcome[0]}（当前基线以刷新状态为准）",completed=True),
                            lambda err: finish_view(f"{describe(err)}\n未自动重试，请只读查询原回执。"))
            except Exception as exc:
                finish_view(f"{describe(exc)}\n请只读查询原回执。")
        self.submit(attempt, "正在只读核对原候选、分支基线和正式槽…", attempt.prepare, reviewed,
                    lambda err: finish_view(f"不能同意续作：{describe(err)}。未上传或自动分叉。"))

    def fork_conflict(self):
        original, live = self.last, self.session
        if (original is None or live is None or self.running is not None or self.owner.pending is not None
                or not self.visible or not original.conflict_pending_seen): return
        try: attempt = PendingFork(original)
        except Exception as err:
            self.output.setText(describe(err)); self.buttons(); return
        self.running = attempt; self.buttons()

        def current():
            return self.session is live and self.last is original and self.running is attempt

        def record(message, *, completed=False):
            receipt = f"{message}\n原 STATE：{attempt.live.state}\n{self.operation_numbers(attempt)}"
            if not current():
                self.discard(attempt)
                self.owner.inform("原窗口已变，不将这笔回执用于其他工作树。\n" + receipt)
                return
            self.last, self.running, self.blocked = attempt, None, True
            self.discard(original)
            self.output.setText(receipt)
            self.blocked = not completed
            self.buttons()
            if completed: self.refresh_status()

        def failed(error):
            if attempt.used:
                record(f"{describe(error)}\n请只读查询原冲突操作和指定后继，不会另发。")
            else:
                if current(): self.output.setText(f"{describe(error)}\n没有产生已受理后继。")
                self.discard(attempt)

        def reviewed(details):
            try:
                if not current() or self.owner.pending is not None:
                    failed("已离开原工作树或原操作；未分叉。"); return
                terms = ("我确认使用上面这份冻结归档。STATE 由本人独占，STORE 不同步、无人改动本次暂存名。"
                         "我同意后继登记完成后切换该工作树的基线，不发布冻结后的编辑。")
                if not confirm(self, "把已冻结的冲突提交发布为新分支", details,
                               attestation=terms, action_text="发布原候选到指定新分支"):
                    failed("未确认；没有制作后继或上传。"); return
                if not current() or self.owner.pending is not None:
                    failed("确认期间原工作树或原操作已改变；未分叉。"); return
                self.output.setText(f"已确认这次冲突分叉，请勿重发。\n{self.operation_numbers(attempt)}")
                def done(result):
                    path, marker = result
                    record(f"该后继的新分支已发布：{path}\n分支：{marker.branch} · 第 1 代。"
                           "已记录完成，当前基线以刷新状态为准。", completed=True)
                self.submit(attempt, "正在复制原冻结归档并发布到指定新分支…", attempt.run, done, failed)
            except Exception as err:
                failed(err)

        self.submit(attempt, "正在只读核实真正的同代竞争及原冻结归档…", attempt.prepare, reviewed, failed)
