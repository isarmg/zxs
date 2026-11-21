"""Native labels, virtualized tables and explicit per-operation consent."""
from PySide6.QtCore import QAbstractTableModel, QModelIndex, Qt
from PySide6.QtWidgets import (QAbstractItemView, QCheckBox, QDialog, QDialogButtonBox,
                               QLabel, QLineEdit, QPlainTextEdit, QPushButton, QTableView, QVBoxLayout)


def label(text=""):
    out = QLabel()
    out.setTextFormat(Qt.TextFormat.PlainText)
    out.setWordWrap(True)
    out.setTextInteractionFlags(Qt.TextInteractionFlag.TextSelectableByMouse)
    out.setText(text)
    return out


def line_diff_dialog(parent, detail, value):
    out = QDialog(parent)
    out.setWindowTitle("选中文件的逐行差异")
    out.resize(840, 600)
    layout = QVBoxLayout(out)
    layout.addWidget(label(detail))
    text = QPlainTextEdit()
    text.setReadOnly(True)
    text.setLineWrapMode(QPlainTextEdit.LineWrapMode.NoWrap)
    text.setPlainText(value)
    layout.addWidget(text)
    controls = QDialogButtonBox(QDialogButtonBox.StandardButton.Close)
    controls.rejected.connect(out.reject)
    layout.addWidget(controls)
    out.finished.connect(lambda _result: text.clear())
    out.finished.connect(lambda _result: out.deleteLater())
    return out


def button(text, callback):
    out = QPushButton(text)
    out.clicked.connect(lambda _checked=False: callback())
    return out


class Rows(QAbstractTableModel):
    def __init__(self, headers, parent):
        super().__init__(parent)
        self.headers, self.rows = tuple(headers), ()

    def replace(self, rows):
        values = tuple(rows)
        self.beginResetModel()
        self.rows = values
        self.endResetModel()

    def rowCount(self, parent=QModelIndex()):
        return 0 if parent.isValid() else len(self.rows)

    def columnCount(self, parent=QModelIndex()):
        return 0 if parent.isValid() else len(self.headers)

    def data(self, index, role=Qt.ItemDataRole.DisplayRole):
        if index.isValid() and role in (Qt.ItemDataRole.DisplayRole, Qt.ItemDataRole.ToolTipRole):
            return self.rows[index.row()][0][index.column()]
        return None

    def headerData(self, section, orientation, role=Qt.ItemDataRole.DisplayRole):
        if role == Qt.ItemDataRole.DisplayRole and orientation == Qt.Orientation.Horizontal:
            return self.headers[section]
        return super().headerData(section, orientation, role)


class Table(QTableView):
    def __init__(self, headers, title):
        super().__init__()
        self.rows = Rows(headers, self)
        self.setModel(self.rows)
        self.setAccessibleName(title)
        self.setSelectionBehavior(QAbstractItemView.SelectionBehavior.SelectRows)
        self.setSelectionMode(QAbstractItemView.SelectionMode.SingleSelection)
        self.setEditTriggers(QAbstractItemView.EditTrigger.NoEditTriggers)
        self.horizontalHeader().setStretchLastSection(True)
        self.verticalHeader().hide()
        self.setAlternatingRowColors(True)

    def item(self):
        indices = self.selectionModel().selectedRows()
        return self.rows.rows[indices[0].row()][1] if indices else None


class ReportPath(QLineEdit):
    """Expose a report row's literal path for ordinary native text selection and copy."""
    def __init__(self):
        super().__init__()
        self.setObjectName("selected-report-path")
        self.setReadOnly(True)
        self.setAccessibleName("所选条目的完整相对路径")
        hint = "选中一行后，可在此全选并复制完整相对路径。"
        self.setAccessibleDescription(hint)
        self.setToolTip(hint)
        self.setPlaceholderText(hint)

    def set_row(self, row):
        self.setText(row.path if row is not None else "")


def confirm(parent, title, detail, *, attestation, action_text="确认并创建"):
    dialog = QDialog(parent)
    dialog.setWindowTitle(title)
    layout = QVBoxLayout(dialog)
    layout.addWidget(label(detail))
    buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Ok | QDialogButtonBox.StandardButton.Cancel)
    buttons.button(QDialogButtonBox.StandardButton.Cancel).setDefault(True)
    gate = QCheckBox(attestation)
    layout.addWidget(gate)
    ok = buttons.button(QDialogButtonBox.StandardButton.Ok)
    ok.setText(action_text)
    ok.setEnabled(False)
    gate.toggled.connect(ok.setEnabled)
    layout.addWidget(buttons)
    buttons.accepted.connect(dialog.accept)
    buttons.rejected.connect(dialog.reject)
    return dialog.exec() == QDialog.DialogCode.Accepted
