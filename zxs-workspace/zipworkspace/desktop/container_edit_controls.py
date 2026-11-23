"""Explicit Linux editing of one selected original in the existing container tab."""
from concurrent.futures import ThreadPoolExecutor
from contextlib import ExitStack, closing
from dataclasses import dataclass, field
from functools import partial
from threading import Event

from PySide6.QtCore import Qt, QTimer
from PySide6.QtWidgets import QComboBox, QHBoxLayout, QMessageBox, QVBoxLayout, QWidget

from ..container_plan import Baseline, FullBody, IgnoredOriginalNameConflict
from ..model import Rejected, new_id
from ..publish import Uncertain
from .container_client import index_for
from .container_connection import NativeWriteProfile
from .container_management import Kind, Phase, Save, _observe, lookup, retry, unresolved, valid_ticket
from .widgets import button, confirm, label


@dataclass(frozen=True, slots=True)
class _Local:
    keep: ExitStack
    owner: object
    active: object
    choice: object
    preview: str
    index: object


def _local_profile(reader, live, point, live_index, initialize):
    from ..linux_edit_broker import EditMountChoice
    from .linux_container_runtime import linux_write_profile
    with ExitStack() as keep:
        choice = EditMountChoice(live, point)
        keep.callback(choice.close)
        preview = choice.review()  # Qualify existing point before any journal creation.
        owner = keep.enter_context(linux_write_profile(
            reader, live, allowed=frozenset({Kind.SAVE, Kind.RENAME, Kind.TAGS, Kind.RECYCLE, Kind.CREATE_BRANCH}),
            confirmed=True, initialize=initialize))
        active = reader.adopt_local(owner.profile, confirmed=True)
        index = index_for(active.operations, live_index)  # use the prior authenticated snapshot
        owner.require_live()
        return _Local(keep.pop_all(), owner, active, choice, preview, index)


def _another_point(owner, point):
    choice = owner.edit_choice(point, confirmed=True)
    try:
        return choice, choice.review()
    except BaseException:
        choice.close()
        raise


def _match_first(offer, ticket):
    if not valid_ticket(ticket) or type(ticket.change) is not Save:
        return False
    target, body = ticket.change.target, ticket.change.body
    return (target.revision == 0 and ticket.base_sequence == 0
            and (target.id, target.name, ticket.submission, ticket.operation, body.node,
                 body.length, body.sha256)
                == (offer.container, offer.name, offer.submission, offer.operation, offer.node,
                    offer.length, offer.sha256))


def _discover_first(local):
    try:
        return local.owner.first_saves(local.active)
    except (Rejected, OSError):
        return None


def _send_first(local, offer):
    if offer.original is not None:
        raise Rejected("this first-file ticket must be checked by its original number")
    ticket = local.owner.prepare_first_save(local.active, offer.submission, offer.container, confirmed=True)
    if not _match_first(offer, ticket):
        raise Rejected("the first-file plan changed after the local selection")
    return _observe(local.active.operations.apply, ticket)


@dataclass(slots=True)
class _Edit:
    choice: object
    live: object
    connection: object
    profile: NativeWriteProfile
    selected: Baseline | None
    import_missing: bool
    granted: Event = field(default_factory=Event)
    controller: object | None = None

    def run(self):
        from .linux_container_controller import LinuxContainerController
        with closing(LinuxContainerController(self.connection, self.profile, self.live)) as controller:
            selected = self.selected
            if selected is None:
                controller.confirm_empty_checkout(confirmed=True)
            else:
                args = dict(container=selected.container_id, branch=selected.branch, node=selected.selected_node,
                            path=selected.name, expected=selected, confirmed=True)
                if self.import_missing:
                    controller.import_checkout(**args, exclusive=True)
                else:
                    controller.verify_existing_checkout(**args)
            self.controller = controller
            self.granted.set()
            try:
                return self.choice.run(confirmed=True, exclusive=True, controller=controller)
            finally:
                self.controller = None

    def capture(self, submission, stopped):
        # The reviewed broker calls this on the same worker, only after the
        # original dispatcher drained and its normal detach was verified.
        if self.controller is None:
            raise Rejected("原编辑会话已结束")
        return self.controller.prepare_cut(stopped, submission=submission)


class ContainerEditControls(QWidget):
    """Qt is only the owner of consent and display; the broker owns the original cut."""
    def __init__(self, host, panel):
        super().__init__(panel)
        self.host, self.panel = host, panel
        self.choice = self.preview = self.local = self.edit = self.pool = None
        self.review = self.run = self.command = self.observation = self.receipt = None
        self.stopped = self.detached = self.announced = False
        self.submission = self.uncertain_container = None
        self.uncertain = False
        self.first, self.first_outcomes = {}, {}
        self.timer = QTimer(self)
        self.timer.timeout.connect(self._poll)
        box, actions = QVBoxLayout(self), QHBoxLayout()
        self.setup = button("接入本机编辑…", self._setup)
        self.empty = button("从本机空目录开始…", lambda: self._begin(False, empty=True))
        self.copy = button("取回所选节点并编辑…", lambda: self._begin(True))
        self.verify = button("用已有原文件编辑…", lambda: self._begin(False))
        self.prepare = button("停写并准备提交…", lambda: self._stop(True))
        self.stop = button("仅停写…", lambda: self._stop(False))
        self.detach = button("正常卸载原卷…", self._detach)
        for item in (self.setup, self.empty, self.copy, self.verify, self.prepare, self.stop, self.detach):
            actions.addWidget(item)
        actions.addStretch()
        box.addLayout(actions)
        self.note = label()
        box.addWidget(self.note)
        self.first_area = QWidget()
        rows = QVBoxLayout(self.first_area)
        first_line = QHBoxLayout()
        self.first_select = QComboBox()
        self.first_select.setAccessibleName("本机新文件待提交")
        self.first_select.currentIndexChanged.connect(lambda *_: self._paint_first())
        self.first_refresh = button("核对本机待提交", self._load_first)
        self.first_apply = button("提交这个新文件…", self._first_apply)
        self.first_status = button("查询新文件原回执", self._first_status)
        self.first_copy = button("复制原传输信息", self._first_copy)
        first_line.addWidget(self.first_select, 1)
        for button_item in (self.first_refresh, self.first_apply, self.first_status, self.first_copy):
            first_line.addWidget(button_item)
        rows.addLayout(first_line)
        self.first_note = label()
        rows.addWidget(self.first_note)
        box.addWidget(self.first_area)
        self.paint()

    def offer(self, choice):
        from ..linux_edit_broker import EditMountChoice
        profile = self.panel._profile
        if (type(profile) is not NativeWriteProfile or type(choice) is not EditMountChoice
                or self.choice is not None or self.edit is not None
                or self.uncertain or self.host.archive.checkout.live is not choice._origin
                or choice._origin.closed or profile._state_selection[0] != choice._origin.state
                or Kind.SAVE not in profile._allowed):
            raise Rejected("只接收本窗口原工作树的既有挂载资格")
        self.choice = choice
        self.note.setText("选中当前分支头或历史 main 节点后，可明确打开本机编辑卷。")
        self.paint()

    def busy(self):
        return self.edit is not None

    def blocks(self, key):
        return (self.edit is not None and self.edit.selected is not None
                and key == self.edit.selected.container_id)

    def finish(self):
        if self.choice is not None:
            self.choice.close()
        if self.local is not None:
            self.local.keep.close()

    def _setup_target(self):
        panel, checkout = self.panel, self.host.archive.checkout
        reader, live = panel._owned, checkout.live
        if (self.edit is not None or self.choice is not None or self.uncertain
                or reader is None or reader.operations is not panel.ops or panel._checking_connection or panel.jobs
                or not self.host._container_edit_ready(live)):
            return None
        if self.local is None:
            return (reader, live) if not reader.writable and panel._profile is None else None
        return (reader, live) if reader is self.local.active and live is self.local.owner.original else None

    def _initial_mode(self, live, point):
        dialog = QMessageBox(self)
        dialog.setWindowTitle("接入本机编辑")
        dialog.setTextFormat(Qt.TextFormat.PlainText)
        dialog.setText(f"原工作目录：{live.work}\n原私有 STATE：{live.state}\n现有空挂载点：{point!r}\n\n"
                       "仅为当前已认证账号接入本机保存、新建分支、重命名、标签与回收操作。"
                       "本步不联网、不挂载。已有登记就使用原登记；只有首次才新建。")
        old = dialog.addButton("使用原登记", QMessageBox.ButtonRole.AcceptRole)
        new = dialog.addButton("首次创建本机登记", QMessageBox.ButtonRole.ActionRole)
        dialog.addButton(QMessageBox.StandardButton.Cancel)
        dialog.setDefaultButton(QMessageBox.StandardButton.Cancel)
        dialog.exec()
        clicked = dialog.clickedButton()
        dialog.deleteLater()
        return True if clicked is new else False if clicked is old else None

    def _setup(self):
        target = self._setup_target()
        if target is None:
            return
        reader, live = target
        # Reuse Workspace's read-only existing-directory selection, accepting
        # it only as a nomination of the distinct private empty mountpoint.
        point = self.host.archive.checkout._directory("选择已有的本人 0700 私有空挂载点", read_only=True)
        def still_selected():
            current = self._setup_target()
            return current is not None and current[0] is reader and current[1] is live

        if not point or not still_selected():
            return
        local = self.local
        if local is None:
            initialize = self._initial_mode(live, point)
            if initialize is None or not still_selected():
                return
            work = partial(_local_profile, reader, live, point, tuple(self.panel.listed.values()), initialize)
        else:
            work = partial(_another_point, local.owner, point)
        epoch = self.panel._epoch

        def selected(result):
            panel = self.panel
            if (panel._epoch != epoch or panel._owned is not reader or panel.jobs
                    or self.local is not local or not self.host._container_edit_ready(live)):
                if local is None:
                    result.keep.close()
                else:
                    result[0].close()
                self.note.setText("原连接或工作树已变；没有接入。")
                return
            if local is None:
                self.local = result
                self.choice, self.preview = result.choice, result.preview
                panel._profile, panel._owned, panel.ops = result.owner.profile, result.active, result.active.operations
                panel._populate(result.index, verified=panel._empty_observed)
            else:
                self.choice, self.preview = result
            self.note.setText("已接入本机编辑资格。")
            panel._paint()
            if local is None:
                self._load_first()

        def refused(_error):
            self.note.setText("未接入本机编辑。请核对原 STATE 登记及现有空挂载点；不会改用其他位置。")
            self.panel._paint()

        self.host.job("正在核对本机编辑资格，未请求挂载…", work, selected, failed=refused)

    def _first_scope(self):
        local = self.local
        return local if local is not None and self.panel._owned is local.active else None

    def _load_first(self):
        local = self._first_scope()
        if local is not None:
            self.panel._start(None, "核对本机待提交", partial(_discover_first, local), self._first_list)

    def _first_list(self, discovered):
        if discovered is None:
            self.first.clear()
            self._show_first("未核对到本机新文件记录。")
            return
        offers, complete = discovered
        self.first = {offer.container: offer for offer in offers
                      if offer.original is None or _match_first(offer, offer.original.ticket)}
        complete = complete and len(self.first) == len(offers)
        self.first_outcomes = {key: value for key, value in self.first_outcomes.items()
                               if key in self.first and _match_first(self.first[key], value.ticket)}
        self._show_first("本机新文件待提交或待查原回执。" if complete and self.first else
                         "部分本机记录尚未核对。" if not complete else "")

    def _show_first(self, message):
        old = self.first_select.currentData()
        prev = self.first_select.blockSignals(True)
        try:
            self.first_select.clear()
            for key, offer in self.first.items():
                self.first_select.addItem(offer.name + " · " + key, key)
            if old in self.first:
                self.first_select.setCurrentIndex(self.first_select.findData(old))
        finally:
            self.first_select.blockSignals(prev)
        self.first_note.setText(message)
        self._paint_first()

    def _first_choice(self):
        local, key = self._first_scope(), self.first_select.currentData()
        offer = self.first.get(key)
        if local is None or offer is None or not self.panel._available(key):
            return None
        return local, offer, self.first_outcomes.get(key, offer.original)

    def _paint_first(self):
        local, selection = self._first_scope(), self._first_choice()
        self.first_area.setVisible(local is not None)
        self.first_select.setVisible(bool(self.first))
        self.first_refresh.setEnabled(local is not None and self.panel._available(None))
        self.first_apply.setVisible(bool(self.first))
        self.first_status.setVisible(bool(self.first))
        prior = selection[2] if selection is not None else None
        can_retry = prior is not None and prior.phase is Phase.ABSENT
        self.first_apply.setText("重试这笔原提交…" if can_retry else "提交这个新文件…")
        self.first_apply.setEnabled(selection is not None and (prior is None or can_retry)
                                    and selection[1].container not in self.panel.listed)
        self.first_status.setEnabled(prior is not None)
        self.first_copy.setVisible(self.panel.can_copy_transfer(prior))
        if selection is not None and selection[2] is not None:
            self.first_note.setText(f"{selection[2].phase.value} · 原操作 {selection[1].operation}")

    def _first_apply(self):
        selection = self._first_choice()
        if selection is None:
            return
        local, offer, prior = selection
        if offer.container in self.panel.listed:
            return
        if prior is None:
            title, action = "提交本机封存的新文件", "提交新文件"
            consent = "只发送这个已封存文件，并使用显示的原操作号。"
            work = partial(_send_first, local, offer)
        elif prior.phase is Phase.ABSENT and _match_first(offer, prior.ticket):
            title, action = "重试这笔原提交", "核对并重试原提交"
            consent = "再次查同一个原操作号；仍明确没有原回执时才发送同一份封存文件。"
            work = partial(retry, local.active.operations, prior, confirmed=True)
        else:
            return
        if (confirm(self, title,
                    f"文件：{offer.name!r} · {offer.length:,} 字节\n容器 {offer.container}\n"
                    f"首个 main 节点 {offer.node}\n原操作 {offer.operation}",
                    attestation=consent, action_text=action)
                and self._first_choice() == selection and offer.container not in self.panel.listed):
            self.panel._start(offer.container, action, work, partial(self._first_result, offer))

    def _first_copy(self):
        selection = self._first_choice()
        if selection is not None:
            self.panel.copy_transfer(selection[2])

    def _first_status(self):
        selection = self._first_choice()
        if selection is not None and selection[2] is not None:
            local, offer, prior = selection
            if not _match_first(offer, prior.ticket):
                return
            self.panel._start(offer.container, "查询新文件原回执",
                              partial(lookup, local.active.operations, prior.ticket),
                              partial(self._first_result, offer))

    def _first_result(self, offer, outcome):
        if not _match_first(offer, outcome.ticket):
            raise Rejected("the new file receipt does not match the selected original")
        key = offer.container
        if outcome.receipt is not None:
            self.panel._empty_observed = False
            self.first.pop(key, None)
            self.first_outcomes.pop(key, None)
            self._show_first(f"已确认首个 main 节点 {outcome.receipt.node} · 原操作 {offer.operation}。"
                             "可再次读取实际容器列表。")
        else:
            self.first_outcomes[key] = outcome
            self._paint_first()

    def _empty_view(self):
        return (self._first_scope() is not None and self.panel._empty_observed and not self.panel.listed
                and not self.panel.jobs and not self.panel._checking_connection)

    def _selected(self):
        panel = self.panel
        key, own = panel._current(), panel._owned
        detail = panel.details.get(key)
        branch, node = panel.branches.currentData(), panel.nodes.currentData()
        if (own is None or own.operations is not panel.ops or not own.writable
                or type(panel._profile) is not NativeWriteProfile
                or Kind.SAVE not in panel._profile._allowed or panel._checking_connection
                or detail is None or detail.container.id != key or key not in panel.listed
                or key in panel.jobs or None in panel.jobs or unresolved(panel.outcomes.get(key))
                or branch is None or node is None or branch.head is None
                or (branch.id != "main" and node.id != branch.head)):
            return None
        return detail, branch, node

    @staticmethod
    def _baseline(selected):
        detail, branch, node = selected
        item = detail.container
        return Baseline(item.id, item.name, item.revision, FullBody(node.length, node.sha256),
                        detail.tags, branch.id, node.id, branch.head)

    def paint(self):
        if (self.local is not None and self.panel._owned is not self.local.active
                and self.edit is None and not self.panel.jobs):
            self.finish()
            self.local = self.choice = self.preview = None
            self.first.clear()
            self.first_outcomes.clear()
            self._show_first("")
        if self.edit is None and self.choice is not None and self.choice._origin.closed:
            self.choice.close()
            self.choice = self.preview = None
        setup = self._setup_target()
        self.setVisible(setup is not None or self.choice is not None or self.edit is not None or bool(self.note.text()))
        self.setup.setVisible(setup is not None)
        self.setup.setText("接入本机编辑…" if self.local is None else "再选一次原编辑卷挂载点…")
        start = self.choice is not None and self.edit is None
        fresh = self._empty_view()
        self.copy.setVisible(start and not fresh)
        self.verify.setVisible(start and not fresh)
        ready = start and self.host._container_edit_ready(self.choice._origin)
        self.empty.setVisible(start and fresh)
        self.empty.setEnabled(ready and fresh)
        self.copy.setEnabled(ready and self._selected() is not None)
        self.verify.setEnabled(ready and self._selected() is not None)
        receipt = self.receipt if self.run is not None and not self.run.done() else None
        stopping = receipt is not None and not self.stopped
        self.prepare.setVisible(stopping)
        self.prepare.setEnabled(stopping and self.command is None and self.panel._owned is self.edit.connection)
        self.stop.setVisible(stopping)
        self.stop.setEnabled(stopping and self.command is None)
        self.detach.setVisible(receipt is not None and self.stopped and not self.detached)
        self.detach.setEnabled(self.command is None)
        self._paint_first()

    def _begin(self, import_missing, *, empty=False):
        choice, panel = self.choice, self.panel
        selected = None if empty else self._selected()
        if (choice is None or self.edit is not None or not self.host._container_edit_ready(choice._origin)
                or (not self._empty_view() if empty else selected is None)):
            return
        try:
            baseline = None if empty else self._baseline(selected)
        except ValueError:
            self.note.setText("所选名称不能直接用作原 WORK 中的单个文件名。")
            return
        self.edit = _Edit(choice, choice._origin, panel._owned, panel._profile, baseline, import_missing)
        preview, self.choice, self.preview = self.preview, None, None  # one use, no retry by path
        self.host._guard_container_edit(True)
        self.note.setText("正在核对本窗口原 WORK 和已授权的空挂载点…")
        try:
            self.pool = ThreadPoolExecutor(max_workers=2, thread_name_prefix="workspace-edit")
            if preview is not None:
                self._allow(preview)
            else:
                self.review = self.pool.submit(choice.review)
                self.timer.start(125)
        except Exception:
            self._end("未能开始核对原编辑卷。")
            return
        panel._paint()

    def _same_selection(self, edit):
        if self.panel._owned is not edit.connection or self.host.archive.checkout.live is not edit.live:
            return False
        if edit.selected is None:
            return self._empty_view()
        selected = self._selected()
        if selected is None:
            return False
        try:
            return self._baseline(selected) == edit.selected
        except ValueError:
            return False

    def _allow(self, native_review):
        edit, selected = self.edit, self.edit.selected
        if selected is None:
            title = "在原卷中新建本机文件"
            detail = "上次已认证的列表为空。只从本窗口新登记且实际为空的 WORK 开始；不采用已有文件。"
        else:
            title = "打开这个节点进行本机编辑"
            transfer = ("原 WORK 中同名占用就停止，不覆盖。" if edit.import_missing
                        else "已有原文件须逐字相同，不改写。")
            intent = ("明确提交时才从历史 main 节点新建分支。"
                      if selected.selected_node != selected.head else "从所示分支头开始编辑。")
            detail = (f"文件：{selected.name!r} · {selected.body.length:,} 字节\n"
                      f"容器 {selected.container_id} · 修订 {selected.revision}\n"
                      f"分支 {selected.branch} · 节点 {selected.selected_node}\n{intent}\n{transfer}\n"
                      "请只改所选文件及本次新建文件，不改卷中其他已有文件。")
        if not self._same_selection(edit) or not confirm(
                self, title, detail + "\n\n" + native_review,
                attestation="我独占这些原位置；只通过该编辑卷改文件，不直接改动或同步 WORK。",
                action_text="核验并挂载原卷") or not self._same_selection(edit):
            self._end("已取消，未取回正文或挂载。")
            return
        self.run = self.pool.submit(edit.run)
        self.note.setText("正在核验所选节点；只在原卷获证后显示位置…")
        self.timer.start(125)

    def _exact_receipt(self, edit):
        if (edit is not None and self.edit is edit and self.run is not None and not self.run.done()
                and self.command is None):
            return self.receipt
        return None

    def _stop(self, prepare):
        edit = self.edit
        receipt = self._exact_receipt(edit)
        if (receipt is None or self.stopped
                or prepare and self.panel._owned is not edit.connection):
            return
        detail = ("正常卸载后按原 WORK 的忽略规则准备提交，不发送。"
                  "忽略项保留在本机；已签出且被忽略的容器保留服务器原版本和名称。"
                  if prepare else "只停写，不准备提交。有改动时会保留原记录待核对。")
        if not confirm(self, "停止这次原卷编辑", f"{receipt.point}\n{detail}",
                       attestation="已保存本机改动，不再从编辑器写入这个卷。",
                       action_text="停写并准备提交" if prepare else "仅停写"):
            return
        if (self._exact_receipt(edit) is not receipt or self.stopped
                or prepare and self.panel._owned is not edit.connection):
            return
        submission = new_id() if prepare else None
        call = (partial(edit.choice.prepare_submit_and_stop, receipt,
                        partial(edit.capture, submission), confirmed=True) if prepare
                else partial(edit.choice.stop, receipt, confirmed=True))
        if self._command("stop", call, "正在确认原卷是否已停止接收新写入…"):
            self.submission = submission

    def _detach(self):
        edit = self.edit
        receipt = self._exact_receipt(edit)
        if receipt is None or not self.stopped or self.detached:
            return
        if confirm(self, "正常卸载这个原卷", f"{receipt.point}\n只对这次已停写的原卷请求正常卸载。",
                   attestation="已关闭使用这块编辑卷的文件、目录和终端。",
                   action_text="正常卸载") and self._exact_receipt(edit) is receipt and not self.detached:
            self._command("detach", partial(edit.choice.detach, receipt, confirmed=True),
                          "正在请求正常卸载这次已停写的原卷…")

    def _command(self, kind, call, message):
        try:
            self.command = (kind, self.pool.submit(call))
        except RuntimeError:
            self.note.setText("原卷操作未能排入本机工作线程。")
            return False
        self.note.setText(message)
        self.paint()
        return True

    def _end(self, message, *, uncertain=False):
        self.timer.stop()
        edit, pool = self.edit, self.pool
        self.edit = self.pool = self.review = self.run = self.command = self.observation = self.receipt = None
        self.stopped = self.detached = self.announced = False
        self.submission = None
        self.uncertain = uncertain
        self.uncertain_container = edit.selected.container_id if uncertain and edit.selected is not None else None
        edit.choice.close()
        if pool is not None:
            pool.shutdown(wait=False)
        self.host._guard_container_edit(uncertain)
        self.note.setText(message)
        self.panel._paint()

    def _poll(self):
        if self.review is not None and self.review.done():
            future, self.review = self.review, None
            self.timer.stop()  # the native modal dialog has its own event loop
            try:
                self._allow(future.result())
            except Exception:
                self._end("未能核对本窗口的原工作目录和空挂载点。")
            self.panel._paint()
            return
        if self.command is not None and self.command[1].done():
            kind, future = self.command
            self.command = None
            try:
                future.result()
            except Exception:
                if kind == "stop":
                    self.submission = None
                    self.note.setText("未确认停写。保留原卷，不会自动再试。")
                else:
                    self.note.setText("未确认正常卸载。请关闭仍占用原卷的程序；不会自动重试。")
            else:
                if kind == "stop":
                    self.stopped = True
                    self.note.setText("已关闭新写入。请关闭所有原卷句柄，再正常卸载。")
                else:
                    self.detached = True
                    self.note.setText("原卷已正常卸载，正在核对本机收尾结果…")
        if self.observation is not None and self.observation.done():
            self.receipt = self.observation.result()
            self.observation = None
        if self.run is not None and self.command is None and self.observation is None:
            if self.run.done():
                try:
                    result = self.run.result()
                except IgnoredOriginalNameConflict as err:
                    self._end(
                        f"无法准备提交：「{err.path}」仍保留给原容器。"
                        f"记录显示原文件曾移到忽略路径「{err.ignored_path}」，同名的新文件不是原件。\n"
                        "本次操作未修改或删除本机文件，提交未创建、未发送。"
                        "本窗口仍暂停旧工作树操作，请保留原件。",
                        uncertain=True,
                    )
                except Exception as err:
                    uncertain = self.edit.granted.is_set() or isinstance(err, Uncertain)
                    if uncertain:
                        point = f"\n原卷位置：{self.receipt.point}" if self.receipt is not None else ""
                        original = f"\n本次请求的本机提交号：{self.submission}" if self.submission else ""
                        self._end("原编辑会话的收尾未确认。本窗口暂停旧工作树操作，请保留原件。" + point + original,
                                  uncertain=True)
                    else:
                        self._end("没有打开编辑卷。请核对所选节点和原文件；没有上传。")
                else:
                    if result is None:
                        self._end("原编辑卷已正常结束，本次没有准备提交。")
                    elif not result.ready:
                        self._end("原编辑卷已正常结束。没有需要上传的改动；忽略项留在本机。")
                    else:
                        self._end(f"已在本机准备原提交 {result.submission}，涉及 {len(result.ready)} 个容器。尚未发送。")
                        self._load_first()
                return
            if self.receipt is None:
                # The broker's lock may cover native checks. Poll the immutable
                # original receipt off GUI, with at most one query in flight.
                choice = self.edit.choice
                self.observation = self.pool.submit(lambda: choice.receipt)
            elif not self.announced:
                self.announced = True
                selected = self.edit.selected
                path = self.receipt.point if selected is None else self.receipt.point / selected.name
                self.note.setText(f"已核对原编辑卷：{path}\n请在这个卷里编辑，普通保存只写本机。")
        self.paint()
