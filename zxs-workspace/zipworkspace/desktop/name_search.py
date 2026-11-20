"""One read-only search dialog, bound to the current verified browser's in-memory index."""
from threading import Event

from PySide6.QtCore import Qt
from PySide6.QtWidgets import QDialog, QHeaderView, QHBoxLayout, QLineEdit, QVBoxLayout

from ..browser import MAX_NAME_QUERY
from .widgets import Table, button, label


class NameSearch(QDialog):
    PAGE = 100
    WORKING = "正在查找本快照的名称…"

    def __init__(self, window, browser, current, go_to):
        super().__init__(window)  # Window.job only disables the tabs, never this Close button.
        self.owner, self.browser, self.current, self.go_to = window, browser, current, go_to
        self.request, self.matches, self.number, self.retired = None, (), 0, False
        self.setWindowTitle("在本快照内按名称查找")
        self.resize(800, 490)
        layout = QVBoxLayout(self)
        layout.addWidget(label("区分大小写，只匹配单个文件或文件夹的名称片段。定位不会打开或另存。"))
        query_row = QHBoxLayout()
        self.term = QLineEdit()
        self.term.setAccessibleName("要查找的名称片段")
        self.term.setPlaceholderText("名称中包含…")
        self.term.returnPressed.connect(self.search)
        self.term.textEdited.connect(self.edited)
        self.find = button("查找", self.search)
        query_row.addWidget(self.term); query_row.addWidget(self.find)
        layout.addLayout(query_row)
        self.message = label()
        layout.addWidget(self.message)
        self.hits = Table(("完整相对路径", "类型", "字节数"), "快照名称查找结果")
        self.hits.setTextElideMode(Qt.TextElideMode.ElideMiddle)
        header = self.hits.horizontalHeader()
        header.setStretchLastSection(False)
        header.setSectionResizeMode(0, QHeaderView.ResizeMode.Stretch)
        for column in (1, 2): header.setSectionResizeMode(column, QHeaderView.ResizeMode.ResizeToContents)
        layout.addWidget(self.hits)
        self.path = QLineEdit()
        self.path.setAccessibleName("所选结果的完整相对路径")
        self.path.setReadOnly(True)
        self.path.setPlaceholderText("选择一行，查看完整相对路径")
        layout.addWidget(self.path)
        nav = QHBoxLayout()
        self.back = button("上一页", lambda: self.page(self.number - 1))
        self.more = button("下一页", lambda: self.page(self.number + 1))
        self.locate = button("定位所选条目", self.activate)
        self.close_button = button("关闭", self.reject)
        for item in (self.find, self.back, self.more, self.locate, self.close_button): item.setAutoDefault(False)
        for item in (self.back, self.more, self.locate, self.close_button): nav.addWidget(item)
        layout.addLayout(nav)
        self.hits.selectionModel().selectionChanged.connect(self.selected)
        self.finished.connect(self.retire)
        self.term.setFocus()
        self.empty()

    def active(self):
        return not self.retired and self.current(self)

    def done(self, code):
        self.retire()
        super().done(code)

    def retire(self, *_):
        if self.retired: return
        self.retired = True
        request, self.request = self.request, None
        if request is not None:
            request.set()
            if self.owner.note.text() == self.WORKING: self.owner.inform("已关闭名称查找。")
        self.term.clear()
        self.message.clear()
        self.empty()

    def empty(self):
        self.matches, self.number = (), 0
        self.hits.rows.replace(())
        self.back.setEnabled(False); self.more.setEnabled(False)
        self.selected()

    def edited(self, _):
        self.empty()
        self.message.clear()

    def search(self):
        if not self.active() or self.request is not None or self.owner.pending is not None: return
        self.empty()
        term = self.term.text()
        if not 1 <= len(term) <= MAX_NAME_QUERY:
            self.message.setText(f"请输入 1 至 {MAX_NAME_QUERY} 个字符。")
            return
        request = self.request = Event()
        browser = self.browser
        self.term.setEnabled(False); self.find.setEnabled(False)
        self.message.setText("正在查找已核验的目录索引…")

        def work():
            # No widget, Selection, archive handle or path in the worker's closure.
            try: return browser.find_names(term, cancelled=request.is_set), None
            except Exception as error: return (), error  # Route failure through the same request gate.

        def done(outcome):
            if self.request is not request or request.is_set() or not self.active(): return
            self.request = None
            self.term.setEnabled(True); self.find.setEnabled(True)
            matches, error = outcome
            if error is not None:
                self.message.setText(f"查找未完成：{error}")
            elif matches:
                self.matches = matches
                self.page(0)
            else:
                self.message.setText("本快照没有包含该名称片段的条目。")
            self.owner.inform("名称查找未完成。" if error is not None else "已查找本快照的目录索引。")

        try:
            future = self.owner.job(self.WORKING, work, done)
            if future is None: raise RuntimeError("有另一项操作正在执行")
        except Exception as error:
            done(((), error))

    def page(self, number):
        count = (len(self.matches) + self.PAGE - 1) // self.PAGE
        if not self.active() or self.request is not None or not 0 <= number < count: return
        self.number = number
        start = number * self.PAGE
        self.hits.rows.replace(((hit.path, "目录" if hit.entry.kind == "directory" else "文件",
                                "" if hit.entry.size_bytes is None else str(hit.entry.size_bytes)), hit)
                               for hit in self.matches[start:start + self.PAGE])
        self.selected()
        self.back.setEnabled(number > 0); self.more.setEnabled(number + 1 < count)
        self.message.setText(f"{len(self.matches):,} 项匹配 · 第 {number + 1}/{count} 页")

    def selected(self, *_):
        hit = self.hits.item() if self.active() and self.request is None else None
        self.locate.setEnabled(hit is not None)
        self.path.setText(hit.path if hit is not None else "")

    def activate(self):
        if not self.active() or self.request is not None or self.owner.pending is not None: return
        hit = self.hits.item()
        if hit is None or not any(hit is original for _, original in self.hits.rows.rows): return
        try: table = self.go_to(self, hit)
        except (ValueError, IndexError) as error:
            self.message.setText(f"无法定位本条原结果：{error}")
            return
        if table is not None:
            self.accept()
            table.setFocus()
