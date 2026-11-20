"""Read only the deliberately selected Store member, presenting its text literally."""
import codecs
from contextlib import nullcontext
from dataclasses import dataclass
import hashlib
import os
import stat
import sys
import unicodedata

from ..archive import stored_member
from ..document import native_identity
from ..model import CHUNK, MAX_CD, MAX_FILE, Rejected

LIMIT = 256 * 1024
UNITS_PER_PAGE, LINES_PER_PAGE = 8192, 256


class _Checked:
    """Use the existing borrowed binary stream when pread is unavailable."""
    def __init__(self, raw, check):
        self.raw, self.check = raw, check

    def tell(self): return self.raw.tell()

    def seek(self, *args): return self.raw.seek(*args)

    def read(self, size):
        self.check()
        data = self.raw.read(size)
        self.check()
        return data


class _At:
    def __init__(self, fd, end, check):
        self.fd, self.end, self.check, self.offset = fd, end, check, 0

    def tell(self): return self.offset

    def seek(self, pos, whence=os.SEEK_SET):
        origin = {os.SEEK_SET: 0, os.SEEK_CUR: self.offset, os.SEEK_END: self.end}[whence]
        if origin + pos < 0: raise Rejected("ZIP 读取的选定偏移无效")
        self.offset = origin + pos
        return self.offset

    def read(self, size):
        self.check()
        if not 0 <= size <= MAX_CD or self.offset + size > self.end:
            raise Rejected("ZIP 超出了原文件的限定读取窗口")
        data = os.pread(self.fd, size, self.offset)
        self.offset += len(data)
        self.check()
        return data


def _stamp(meta):
    return (*native_identity(meta, stat.S_IFREG), meta.st_size, meta.st_mtime_ns, meta.st_ctime_ns)


def _showable(text):
    # Permit CR only through the caller's explicit CRLF conversion. Reject
    # control/spoofing categories and common zero-width presentation selectors.
    for ch in text:
        if ch in "\n\t": continue
        scalar = ord(ch)
        if (unicodedata.category(ch) in ("Cc", "Cf", "Co", "Cn", "Cs", "Zl", "Zp") or
                scalar in (0x034f, 0x115f, 0x1160, 0x3164, 0xffa0) or
                0x180b <= scalar <= 0x180f or 0xfe00 <= scalar <= 0xfe0f or
                0xe0100 <= scalar <= 0xe01ef):
            return False
    return True


def _decode(captured, size):
    if size <= LIMIT:
        return captured.decode("utf-8").replace("\r\n", "\n"), size
    decoder = codecs.getincrementaldecoder("utf-8")("strict")
    text = decoder.decode(captured[:LIMIT])
    pending, _ = decoder.getstate()
    used = LIMIT - len(pending)
    if pending:
        # Check the single straddling scalar before dropping it. An invalid
        # continuation or inadmissible scalar may not disappear silently.
        width = 2 if pending[0] < 0xe0 else 3 if pending[0] < 0xf0 else 4
        tail = decoder.decode(captured[LIMIT:LIMIT + width - len(pending)], final=True)
        if len(tail) != 1 or not _showable(tail): raise Rejected("非可显示的 UTF-8 文本")
    if text.endswith("\r"):
        if captured[used:used + 1] != b"\n": raise Rejected("文本含孤立回车")
        text = text[:-1]
        used -= 1
    return text.replace("\r\n", "\n"), used


def _pages(text, check):
    out = []
    start = units = lines = 0
    for i, ch in enumerate(text):
        if i % 1024 == 0: check()
        cost, line = 1 + (ord(ch) > 0xffff), int(ch == "\n")
        if units + cost > UNITS_PER_PAGE or lines + line > LINES_PER_PAGE:
            out.append(text[start:i])
            start, units, lines = i, 0, 0
        units += cost
        lines += line
    out.append(text[start:])
    return tuple(out)


@dataclass(frozen=True, slots=True)
class TextPreview:
    path: str
    bytes_total: int
    bytes_shown: int
    pages: tuple[str, ...]


def prepare(selected, directory, entry, *, check):
    if (selected.held is None or entry.kind != "file" or
            selected.browser.entry(directory, entry.name) is not entry or
            type(entry.size_bytes) is not int or not 0 <= entry.size_bytes <= MAX_FILE):
        raise Rejected("只能预览当前目录页明确选择的已核普通成员")
    member = f"{directory}/{entry.name}" if directory else entry.name
    size, observed = entry.size_bytes, selected.observed
    policy = nullcontext()
    resident = None
    if sys.platform == "darwin":
        from ..macos_read import no_materialization, resident
        policy = no_materialization()
    # Retire the borrowed descriptor only after the final checks and (on
    # macOS) restore this worker's original materialization policy on exit.
    with policy, selected.held.read_lease() as fd:
        def same_original():
            check()
            if resident is not None: resident(selected.path.name, fd)
            if (_stamp(os.fstat(fd)) != observed or
                    _stamp(selected.path.stat(follow_symlinks=False)) != observed):
                raise Rejected("归档原件已改变；未重新打开同名包")
        same_original()
        capture, digest, read = bytearray(), hashlib.sha256(), 0
        budget = min(size, LIMIT + 3)
        # Linux/macOS use pread so view work cannot move the description's
        # shared seek cursor (e.g. an independent Linux FUSE process).
        raw = (nullcontext(_At(fd, observed[2], check)) if hasattr(os, "pread")
               else os.fdopen(fd, "rb", closefd=False))
        with raw as opened, stored_member(_Checked(opened, check), observed[2], selected.browser.commit, member, size) as body:
            while True:
                check()
                block = body.read(CHUNK)
                check()
                if not block: break
                read += len(block)
                if read > size: raise Rejected("原成员超出已核的长度")
                digest.update(block)
                if len(capture) < budget: capture += block[:budget - len(capture)]
        if read != size or digest.hexdigest() != entry.sha256:
            raise Rejected("实读成员与原核验的长度或 SHA-256 不符")
        same_original()
        try: text, shown = _decode(capture, size)
        except UnicodeError as err: raise Rejected("成员通过内容核验，但所选范围不是有效 UTF-8 文本") from err
        if not _showable(text): raise Rejected("成员通过内容核验，但所选范围含不可显示的控制字符")
        pages = _pages(text, check)
        same_original()
        return TextPreview(member, size, shown, pages)


def dialog(parent, preview):
    """Build one native scrolling, noneditable page; never parse rich text or links."""
    from PySide6.QtCore import Qt
    from PySide6.QtWidgets import QDialog, QHBoxLayout, QPlainTextEdit, QVBoxLayout
    from .widgets import button, label

    out = QDialog(parent)
    out.setWindowTitle("已核验的纯文本")
    out.setAttribute(Qt.WidgetAttribute.WA_DeleteOnClose)
    out.resize(760, 540)
    box = QVBoxLayout(out)
    description = ("核验后的全文" if preview.bytes_shown == preview.bytes_total
                   else f"只显示前 {preview.bytes_shown:,} 字节，后续内容未作文本判断")
    box.addWidget(label(f"{preview.path}\n已核整项 {preview.bytes_total:,} B · {description}"))
    page = label()
    box.addWidget(page)
    text = QPlainTextEdit()
    text.setReadOnly(True)
    text.setContextMenuPolicy(Qt.ContextMenuPolicy.NoContextMenu)
    text.setAcceptDrops(False)
    text.setTextInteractionFlags(Qt.TextInteractionFlag.NoTextInteraction)
    box.addWidget(text)
    bar = QHBoxLayout()
    box.addLayout(bar)
    index, pages, total = 0, preview.pages, preview.bytes_total

    def draw(delta):
        nonlocal index
        if not pages: return
        index = min(max(index + delta, 0), len(pages) - 1)
        text.setPlainText(pages[index])
        text.verticalScrollBar().setValue(0)
        page.setText(f"{index + 1}/{len(pages)} 页" if total else "空文本")
        back.setEnabled(index > 0)
        more.setEnabled(index + 1 < len(pages))

    back = button("上一页", lambda: draw(-1))
    more = button("下一页", lambda: draw(1))
    for control in (back, more, button("关闭", out.reject)): bar.addWidget(control)
    def clear(_answer):
        nonlocal pages
        pages = ()
        text.clear()

    out.finished.connect(clear)
    draw(0)
    return out
