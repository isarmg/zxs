"""Native labels, virtualized tables and explicit per-operation consent."""
from PySide6.QtCore import QAbstractTableModel, QModelIndex, Qt
from PySide6.QtWidgets import (QAbstractItemView, QCheckBox, QDialog, QDialogButtonBox,
                               QLabel, QPushButton, QTableView, QVBoxLayout)


def label(text=""):
    out = QLabel()
    out.setTextFormat(Qt.TextFormat.PlainText)
    out.setWordWrap(True)
    out.setTextInteractionFlags(Qt.TextInteractionFlag.TextSelectableByMouse)
    out.setText(text)
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


def confirm(parent, title, detail, *, attestation):
    dialog = QDialog(parent)
    dialog.setWindowTitle(title)
    layout = QVBoxLayout(dialog)
    layout.addWidget(label(detail))
    buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Ok | QDialogButtonBox.StandardButton.Cancel)
    buttons.button(QDialogButtonBox.StandardButton.Cancel).setDefault(True)
    gate = QCheckBox(attestation)
    layout.addWidget(gate)
    ok = buttons.button(QDialogButtonBox.StandardButton.Ok)
    ok.setText("确认并创建")
    ok.setEnabled(False)
    gate.toggled.connect(ok.setEnabled)
    layout.addWidget(buttons)
    buttons.accepted.connect(dialog.accept)
    buttons.rejected.connect(dialog.reject)
    return dialog.exec() == QDialog.DialogCode.Accepted
