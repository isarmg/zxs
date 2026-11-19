"""One explicit, non-forking save for a live CLI checkout. A query never uploads."""
from PySide6.QtWidgets import QHBoxLayout, QVBoxLayout, QWidget

from .commit_choice import CommitChoice
from .retry_choice import PendingRetry
from .widgets import button, confirm, label


class CommitControls(QWidget):
    def __init__(self, owner, refresh):
        super().__init__()
        self.owner, self.refresh_status = owner, refresh
        self.session = self.last = self.running = None
        self.blocked = False
        box = QVBoxLayout(self); row = QHBoxLayout()
        self.save = button("提交本次工作目录…", self.begin)
        self.query = button("查询该次提交的原回执", self.inspect)
        self.retry = button("核对并续作原待决操作…", self.resume)
        row.addWidget(self.save); row.addWidget(self.query); box.addLayout(row)
        box.addWidget(self.retry)
        self.output = label(); box.addWidget(self.output)
        self.set_session(None)

    def buttons(self):
        self.save.setEnabled(self.session is not None and self.running is None and not self.blocked)
        free = self.session is not None and self.running is None and self.last is not None
        self.query.setEnabled(free)
        self.retry.setEnabled(free and self.last.pending_seen and not self.last.conflict)

    def set_session(self, live):
        if self.last is not None and self.last is not self.running: self.last.release()
        self.session, self.last, self.blocked = live, None, False
        self.output.setText("只提交本窗口刚创建的原工作树，不从 STATE 路径补领。")
        self.buttons()

    def discard(self, job):
        if self.last is job: self.last = None
        try: job.release()
        except (OSError, ValueError) as error: self.owner.inform(str(error))
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
        original = self.session
        if original is None or self.running is not None or self.blocked or self.owner.pending is not None: return
        try: job = CommitChoice(original)
        except Exception as err:
            self.output.setText(str(err)); return
        self.running = job; self.buttons()

        def refused(err):
            if self.session is original: self.output.setText(str(err))
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
                if self.session is not original or not confirm(self, "确认提交整个工作目录", detail,
                        attestation="我已停止所有工作树写入者。STATE 由本人独占，STORE 不同步、不会有其他程序改动本次上传暂存名。",
                        action_text="确认并提交"):
                    refused("未确认，未冻结或提交。"); return
                if self.session is not original:
                    refused("原窗口工作树已切换，未提交。"); return
                if self.last is not None:
                    self.last.release(); self.last = None
                self.output.setText(f"已确认原操作 {job.operation}；不要重复提交。")
                def failed(error):
                    if not job.used:
                        refused(error); return
                    after_submit(f"{error}\n尚不能从此次返回确定结果；只读查询本次原回执。", uncertain=True)
                def done(result):
                    path, marker = result
                    after_submit(f"新快照已存入：{path}\n分支：{marker.branch} · 第 {marker.generation} 代\n原节点：{marker.operation}",
                                 uncertain=False)
                self.submit(job, "正在冻结、核对并发布本次工作树…", job.run, done, failed)
            except Exception as err:
                if job.used: after_submit(f"{err}\n结果待核。", uncertain=True)
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
            self.output.setText(f"{text}\nSTATE：{job.live.state}\n操作号：{job.operation}")
        self.submit(job, "正在只读查询本次原操作…", job.result,
                    lambda result: end(*result), lambda error: end("unknown", f"原结果未知：{error}。未发布或重试。"))

    def resume(self):
        original, live = self.last, self.session
        if (not original or live is None or self.running is not None or self.owner.pending is not None
                or not original.pending_seen or original.conflict): return
        try: attempt = PendingRetry(original)
        except Exception as err:
            self.output.setText(str(err)); self.buttons(); return
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
                ack_only = attempt.ack_fd is not None
                if not confirm(self, "只处理原冻结操作", details,
                        attestation="原 STATE 仍由本人独占，STORE 不同步、无人改动本次暂存名。我确认只处理所列原操作。",
                        action_text="仅确认原回执" if ack_only else "续作原冻结候选"):
                    finish_view("未确认，未续作。再次尝试须先查询原回执。"); return
                if self.session is not live or self.last is not original:
                    finish_view("工作树已切换，未提交续作。"); return
                self.submit(attempt, "正在处理这笔原冻结操作…", attempt.run,
                            lambda outcome: finish_view(f"已确认原操作的正式槽：{outcome[0]}",completed=True),
                            lambda err: finish_view(f"{err}\n未自动重试，请只读查询原回执。"))
            except Exception as exc:
                finish_view(f"{exc}\n请只读查询原回执。")
        self.submit(attempt, "正在只读核对原候选、分支基线和正式槽…", attempt.prepare, reviewed,
                    lambda err: finish_view(f"不能同意续作：{err}。未上传或自动分叉。"))
