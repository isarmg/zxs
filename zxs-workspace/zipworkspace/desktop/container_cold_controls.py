"""Exact existing-transfer recovery inside a read-only Linux connection."""
from contextlib import ExitStack
from dataclasses import dataclass, field
from functools import partial
from pathlib import Path

from PySide6.QtGui import QGuiApplication
from PySide6.QtWidgets import QDialog, QDialogButtonBox, QFormLayout, QHBoxLayout, QLineEdit, QVBoxLayout, QWidget

from .container_cold_transfer import reopen_original_transfer
from .container_management import CreateBranch, Outcome, Phase, id_ok, kind, valid_ticket
from .widgets import button, confirm, label


def _reference(state, container, submission, operation):
    if not state.isprintable() or len(state.encode("utf-8", "surrogatepass")) > 4096:
        return None
    path = Path(state)
    if (path.anchor != "/" or len(path.parts) < 2 or str(path) != state or ".." in path.parts
            or path.parts[1] in ("dev", "proc", "sys") or not all(map(id_ok, (container, submission, operation)))):
        return None
    return path, container, submission, operation


def copy_reference(state, ticket):
    if not valid_ticket(ticket):
        return False
    contact = _reference(str(state), ticket.change.target.id, ticket.submission, ticket.operation)
    if contact is None:
        return False
    QGuiApplication.clipboard().setText("\n".join(map(str, contact)))
    return True


def _input(parent):
    dialog = QDialog(parent)
    dialog.setWindowTitle("按原号恢复传输")
    form = QFormLayout(dialog)
    form.addRow(label("只核对已登记的原 STATE 和原票据。先查询原回执；此步不会上传或挂载。"))
    labels = "原私有 STATE（完整路径）", "容器号", "本机提交号", "原操作号"
    fields = tuple(QLineEdit() for _ in labels)
    for title, edit in zip(labels, fields):
        form.addRow(title, edit)

    def values():
        return _reference(*(edit.text() for edit in fields))

    def paste():
        raw = QGuiApplication.clipboard().text()
        lines = raw.splitlines() if len(raw) <= 8192 else ()
        if len(lines) == 4 and _reference(*lines) is not None:
            for edit, value in zip(fields, lines):
                edit.setText(value)
            pasted.setText("已填入原号。请核对后再查。")
        else:
            pasted.setText("剪贴板中没有完整的原传输信息。")

    form.addRow(button("粘贴原传输信息", paste))
    pasted = label()
    form.addRow(pasted)
    choices = QDialogButtonBox(QDialogButtonBox.StandardButton.Ok | QDialogButtonBox.StandardButton.Cancel)
    query = choices.button(QDialogButtonBox.StandardButton.Ok)
    query.setText("查询原回执")
    query.setEnabled(False)
    for edit in fields:
        edit.textChanged.connect(lambda *_: query.setEnabled(values() is not None))
    choices.accepted.connect(dialog.accept)
    choices.rejected.connect(dialog.reject)
    form.addRow(choices)
    try:
        return values() if dialog.exec() == QDialog.DialogCode.Accepted else None
    finally:
        dialog.deleteLater()


@dataclass(slots=True)
class _Case:
    reader: object
    reference: tuple
    keep: ExitStack = field(default_factory=ExitStack)
    scope: object | None = None

    @property
    def container(self):
        return self.reference[1]

    def close(self):
        self.scope = None
        self.keep.close()

    def query(self):
        try:
            if self.scope is None:
                state, container, submission, operation = self.reference
                self.scope = self.keep.enter_context(reopen_original_transfer(
                    self.reader, state, container=container, submission=submission,
                    operation=operation, confirmed=True))
            return self.scope.status()
        except Exception:
            self.close()
            return None

    def retry(self, prior):
        try:
            return self.scope.retry(prior, confirmed=True)
        except Exception:
            self.close()
            return None


class ColdTransferControls(QWidget):
    """One selected transfer, independently of WORK and the live container list."""
    def __init__(self, panel):
        super().__init__(panel)
        self.panel, self.case, self.outcome = panel, None, None
        box, row = QVBoxLayout(self), QHBoxLayout()
        self.choose = button("按原号恢复传输…", self._choose)
        self.query = button("查询同一原回执", self._query)
        self.retry = button("重试同一原操作…", self._retry)
        for action in (self.choose, self.query, self.retry):
            row.addWidget(action)
        row.addStretch()
        box.addLayout(row)
        self.note = label()
        box.addWidget(self.note)

    def _reader(self):
        owner = self.panel._owned
        if (owner is not None and owner._listed and not owner.writable and not self.panel._checking_connection
                and owner.operations is self.panel.ops):
            return owner
        return None

    def _current(self):
        case = self.case
        return (case if case is not None and case.reader is self._reader()
                and self.panel._available(case.container) else None)

    def paint(self):
        reader = self._reader()
        if self.case is not None and self.case.reader is not reader and self.case.container not in self.panel.jobs:
            self.finish()
        self.setVisible(reader is not None)
        selected = self._current()
        idle = self.case is None or self.case.container not in self.panel.jobs
        self.choose.setEnabled(reader is not None and idle)
        self.query.setVisible(self.case is not None and self.case.reader is reader)
        self.query.setEnabled(selected is not None)
        absent = self.outcome is not None and self.outcome.phase is Phase.ABSENT
        self.retry.setVisible(selected is not None and absent)
        self.retry.setEnabled(selected is not None and absent)

    def finish(self):
        case, self.case, self.outcome = self.case, None, None
        if case is not None:
            case.close()
        self.note.clear()

    def _choose(self):
        reader, epoch = self._reader(), self.panel._epoch
        if reader is None or (self.case is not None and self.case.container in self.panel.jobs):
            return
        ref = _input(self)
        if (ref is None or reader is not self._reader() or epoch != self.panel._epoch
                or (self.case is not None and self.case.container in self.panel.jobs)
                or not self.panel._available(ref[1])):
            return
        self.finish()
        self.case = _Case(reader, ref)
        self._query()

    def _query(self):
        case = self._current()
        if case is not None:
            self._begin(case, case.query, "正在查询这个原操作的回执…")

    def _retry(self):
        case, prior = self._current(), self.outcome
        if case is None or prior is None or prior.phase is not Phase.ABSENT:
            return
        original = prior.ticket
        if (confirm(self, "重试同一原操作",
                    f"{kind(original.change).value} · {original.change.target.name!r}\n"
                    f"容器 {original.change.target.id}\n原操作 {original.operation}",
                    attestation="仍明确没有原回执时，才按这个原号发送同一笔。",
                    action_text="核对并重试")
                and self._current() is case and self.outcome is prior):
            self._begin(case, partial(case.retry, prior), "正在再次核对原回执…")

    def _begin(self, case, work, message):
        self.outcome = None  # only a new observed ABSENT can enable another retry
        self.note.setText(message)
        self.panel._start(case.container, "原传输", work, partial(self._result, case))

    def _result(self, case, outcome):
        if self.case is not case or case.reader is not self._reader():
            return
        if (type(outcome) is not Outcome or not valid_ticket(outcome.ticket)
                or (outcome.ticket.change.target.id, outcome.ticket.submission, outcome.ticket.operation) != case.reference[1:]):
            self.outcome = None
            self.note.setText(f"未取得原传输结果。请核对原 STATE 和原操作 {case.reference[3]}。")
            return
        self.outcome = outcome
        receipt = outcome.receipt
        extra = ""
        if receipt is not None:
            if receipt.recycled:
                extra = " · 已移入专用回收盘"
                if receipt.recycle_source_cleanup_pending is True:
                    extra += "（原盘清理待完成）"
                elif receipt.recycle_source_cleanup_pending is None:
                    extra += "（原盘清理未核对）"
            elif type(outcome.ticket.change) is CreateBranch:
                extra = f" · 新分支 {receipt.branch} · 继承原节点 {receipt.node}"
            elif receipt.node is not None:
                extra = f" · 节点 {receipt.node}"
        self.note.setText(f"{outcome.phase.value}{extra} · 原操作 {outcome.ticket.operation}")
