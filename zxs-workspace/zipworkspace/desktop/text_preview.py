"""Read only the deliberately selected Store member, presenting its text literally."""
from bisect import bisect_right
import codecs
from contextlib import ExitStack, closing, contextmanager, nullcontext
from dataclasses import dataclass
from difflib import unified_diff
import hashlib
import os
import re
import stat
from struct import Struct
from threading import Event
import sys
import unicodedata

from ..archive import stamp, stored_member
from ..comparison import COMPARE_BYTES
from ..document import check_relative, native_identity, relative_document
from ..freeze import _dir_flags
from ..lifecycle import MissingBaseline, _base_leaf, _idle_live_base
from ..model import CHUNK, MAX_CD, MAX_FILE, Rejected, member_name
from ..private_freeze import mount_id
from ..private_stage import same_mount
from .selection import PlainSelection, Selection

LIMIT = 256 * 1024
COMPARE_LINES = 1024
UNITS_PER_PAGE, LINES_PER_PAGE = 8192, 256
# At 128 GiB this is 131,072 × 37 B (less than 4.7 MiB). There is
# deliberately no table per line or per rendered page. Each non-final page
# within a section consumes at least 256 source bytes (line cap or unit cap),
# with at most one short final page per block, so a uint32 page number suffices.
_BLOCK = Struct("<IB32s")  # first zero-based page, raw bytes carried in, fixed block SHA-256
_ASCII_CONTROL = re.compile(r"[\x00-\x08\x0b-\x1f\x7f]")


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


def _binary_reader(fd, end, check):
    # POSIX pread never moves a shared cursor. Without pread the caller
    # already owns a serialized _DisplayFD read lease and restores its cursor.
    return (nullcontext(_At(fd, end, check)) if hasattr(os, "pread")
            else os.fdopen(fd, "rb", buffering=0, closefd=False))


def _showable(text):
    # Permit CR only through the caller's explicit CRLF conversion. Reject
    # control/spoofing categories and common zero-width presentation selectors.
    if text.isascii(): return _ASCII_CONTROL.search(text) is None
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


def _page_ranges(text, check):
    # Fast paths for a long ASCII line and all-linefeed segments. The
    # mixed-text path below has the same UTF-16 and newline geometry.
    if text and text.isascii():
        lines = text.count("\n")
        if not lines or lines == len(text):
            width = LINES_PER_PAGE if lines else UNITS_PER_PAGE
            for start in range(0, len(text), width):
                check()
                yield start, min(start + width, len(text))
            return
    start = units = lines = 0
    for i, ch in enumerate(text):
        if i % 1024 == 0: check()
        cost, line = 1 + (ord(ch) > 0xffff), int(ch == "\n")
        if units + cost > UNITS_PER_PAGE or lines + line > LINES_PER_PAGE:
            yield start, i
            start, units, lines = i, 0, 0
        units += cost
        lines += line
    yield start, len(text)


def _pages(text, check):
    return tuple(text[a:b] for a, b in _page_ranges(text, check))


@dataclass(frozen=True, slots=True)
class TextPreview:
    path: str
    bytes_total: int
    bytes_shown: int
    pages: tuple[str, ...]


def _address(selected, directory, entry):
    if isinstance(selected, PlainSelection):
        index = selected.index
    elif isinstance(selected, Selection):
        index = selected.browser
    else: raise Rejected("没有已核验的归档目录")
    if (selected.held is None or selected.held.closed or entry.kind != "file" or
            index.entry(directory, entry.name) is not entry or
            type(entry.size_bytes) is not int or not 0 <= entry.size_bytes <= MAX_FILE):
        raise Rejected("只能预览当前目录页明确选择的已核普通成员")
    return f"{directory}/{entry.name}" if directory else entry.name


@contextmanager
def _held_input(selected, check):
    observed = selected.observed
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
        # Linux/macOS use pread rather than moving another reader's cursor.
        # Windows uses the serialized CRT lease without implicit read-ahead.
        with _binary_reader(fd, observed[2], check) as opened:
            yield _Checked(opened, check), same_original
            same_original()


def prepare(selected, directory, entry, *, check, indexed=False):
    member = _address(selected, directory, entry)
    size, end = entry.size_bytes, selected.observed[2]
    # A previously verified ordinary ZIP contributes its original manifest and
    # member SHA, never a managed marker from its comment.
    marker = selected.browser.commit if isinstance(selected, Selection) else None
    if indexed and size <= LIMIT: raise Rejected("小文本无需另建分页索引")
    capture, digest, read = bytearray(), hashlib.sha256(), 0
    build = _Build(size) if indexed else None
    budget = 0 if indexed else min(size, LIMIT + 3)
    with _held_input(selected, check) as (original, same_original):
        with stored_member(original, end, marker, member, size, locate=True) as (body, start):
            while True:
                check()
                block = body.read(CHUNK)
                check()
                if not block: break
                read += len(block)
                if read > size: raise Rejected("原成员超出已核的长度")
                digest.update(block)
                if len(capture) < budget: capture += block[:budget - len(capture)]
                if build is not None:
                    try: build.add(block, read, check)
                    except (UnicodeError, _NotText):
                        # Finish CRC and SHA even when the text profile failed.
                        build = None
        if read != size or digest.hexdigest() != entry.sha256:
            raise Rejected("实读成员与原核验的长度或 SHA-256 不符")
        same_original()
        if indexed:
            if build is None:
                raise Rejected("整项内容核验通过，但全文含无效 UTF-8 或不可显示的字符")
            return build.finish(selected, directory, entry, member, start)
        try: text, shown = _decode(capture, size)
        except UnicodeError as err: raise Rejected("成员通过内容核验，但所选范围不是有效 UTF-8 文本") from err
        if not _showable(text): raise Rejected("成员通过内容核验，但所选范围含不可显示的控制字符")
        pages = _pages(text, check)
        same_original()
        return TextPreview(member, size, shown, pages)


def can_compare_text(row):
    return (row is not None and (row.change, row.before, row.after) == ("changed", "file", "file")
            and all(type(n) is int and 0 <= n <= COMPARE_BYTES for n in (row.before_bytes, row.after_bytes)))


def _split_comparison(text):
    line_count = text.count("\n") + int(bool(text) and not text.endswith("\n"))
    if line_count > COMPARE_LINES:
        raise Rejected("文件已核验，但逐行差异每侧最多 1,024 行")
    return text.splitlines(keepends=True)


def _unified(sides, titles, observer):
    observer.begin("compare")
    display = []
    for line in unified_diff(*sides, fromfile=titles[0], tofile=titles[1]):
        observer.check()
        display.append(line if line.endswith("\n") else line + "\n\\ 此行末尾没有换行\n")
    return "".join(display) if display else "两份原件的字节不同；统一换行后，显示的文字相同。"


def compare_text(left, right, row, *, observer):
    """Two small complete members from precisely the original checked views."""
    if not can_compare_text(row):
        raise Rejected("逐行对照要求同一路径的两个普通文本文件，且每侧不超过 64 KiB")
    folder, _, name = row.path.rpartition("/")
    entries = [view.browser.entry(folder, name) for view in (left, right)]
    for entry, size in zip(entries, (row.before_bytes, row.after_bytes)):
        if entry is None or entry.kind != "file" or entry.size_bytes != size:
            raise Rejected("所选差异不再对应两份原成员")
    if entries[0].sha256 == entries[1].sha256:
        raise Rejected("这两条原成员没有字节差异")
    observer.begin("members", row.before_bytes + row.after_bytes)
    sides = []
    for view, entry in zip((left, right), entries):
        read = prepare(view, folder, entry, check=observer.check)
        if read.path != row.path or read.bytes_shown != entry.size_bytes:
            raise Rejected("没有取得原文件的完整文本，未生成差异")
        observer.advance(entry.size_bytes)
        sides.append(_split_comparison("".join(read.pages)))
    display = _unified(sides, ("当前快照", "所选快照"), observer)
    observer.begin("identity")
    for view in (left, right):
        with _held_input(view, observer.check): pass  # Metadata only; no re-open or extra member read.
    observer.check()
    return display


def _verified_small(read, fact, observer):
    size, expected = fact
    data, hash_ = bytearray(), hashlib.sha256()
    while len(data) < size:
        observer.check()
        n = min(65536, size-len(data))
        piece = read(n)
        observer.check()
        if not 0 < len(piece) <= n: raise Rejected("不能完整读取本次对照中的原件")
        data += piece
        hash_.update(piece)
        observer.advance(len(piece))
    if read(1) or hash_.hexdigest() != expected:
        raise Rejected("内容与本次对照中的原 SHA-256 不符；请刷新对照")
    observer.check()
    return data


def _small_lines(data):
    try: text = data.decode("utf-8").replace("\r\n", "\n")
    except UnicodeError as error: raise Rejected("两个原件已核验，但至少一个不是严格 UTF-8 文本") from error
    if not _showable(text): raise Rejected("内容含不可显示控制字符，未交逐行差异")
    return _split_comparison(text)


def compare_folder_text(left, report, row, *, observer):
    """After separate consent, verify only the original pair from the folder report."""
    if sys.platform == "darwin":
        from ..macos_read import in_disposable_thread
        from ..macos_folder import MacFolderText
        if isinstance(report.text, MacFolderText):
            # The context, ZIP/peer reads and final rechecks must execute on
            # the *same* short-lived policy owner, not yield to a shared worker.
            return in_disposable_thread(_compare_folder_text, left, report, row, observer)
    return _compare_folder_text(left, report, row, observer)


def _compare_folder_text(left, report, row, observer):
    if not can_compare_text(row) or report.text is None:
        raise Rejected("该次对照没有这对原件的逐行核验凭据")
    observer.begin("identity")
    with report.text.open_pair(left, row, observer.check) as (original, read_local, left_sha, right_sha):
        observer.begin("members", row.before_bytes + row.after_bytes)
        end = left.observed[2]
        with _binary_reader(original, end, observer.check) as raw:
            with stored_member(_Checked(raw, observer.check), end, left.browser.commit, row.path, row.before_bytes) as body:
                archived = _verified_small(body.read, (row.before_bytes, left_sha), observer)
        current = _verified_small(read_local, (row.after_bytes, right_sha), observer)
        display = _unified((_small_lines(archived), _small_lines(current)), ("当前快照", "本次本机文件"), observer)
        observer.begin("identity")
    return display


class WorktreeTextPair:
    """One bound baseline member and one relative working file, never a writer."""
    def __init__(self, live, evidence, row):
        if (not can_compare_text(row) or not isinstance(evidence, MissingBaseline)
                or evidence.instance != live.instance or evidence.path != live.store / _base_leaf(evidence.basis)):
            raise Rejected("不是本窗口对照中的同一路径小文本")
        self.facts = evidence.text_pair(row)
        self.parts, self.member = member_name(row.path), row.path
        self.observed, self.basis, self.marker = evidence.observed, evidence.basis, evidence.marker
        self.leaf = evidence.path.name
        self.closed = False
        self.stack = ExitStack()
        try:
            self.live = self.stack.enter_context(closing(live.borrow()))
            self.source = evidence.duplicate()
            self.stack.callback(os.close, self.source)
        except BaseException:
            self.close()
            raise

    def close(self):
        if not self.closed:
            self.closed = True
            self.stack.close()

    def _check_basis(self):
        if _idle_live_base(self.live) != self.basis:
            raise Rejected("原基线已改变；请重新查看工作树对照")

    def _check_archive(self):
        # O_PATH supplies only metadata for the original relative STORE name;
        # all actual archive bytes come from the still-held data descriptor.
        store = self.live.store_fd
        named = os.open(self.leaf, os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=store)
        try:
            if (_stamp(os.fstat(named)) != self.observed or _stamp(os.fstat(self.source)) != self.observed
                    or mount_id(named) != mount_id(self.source) or mount_id(named) != mount_id(store)):
                raise Rejected("登记基线已不是本次持有的原归档")
        finally: os.close(named)

    @contextmanager
    def _working(self, check):
        root = self.live.work_fd
        mount = same_mount(self.live.state_fd, root)
        parent, opened = root, []
        with ExitStack() as owned:
            for part in self.parts[:-1]:
                check()
                fd = os.open(part, _dir_flags(), dir_fd=parent)
                owned.callback(os.close, fd)
                if mount_id(fd) != mount: raise Rejected("工作文件父目录越过原挂载")
                opened.append((parent, part, fd))
                parent = fd
            leaf = self.parts[-1]
            with relative_document(parent, leaf) as doc:
                before = stamp(doc)

                def identical():
                    if mount_id(doc.fileno()) != mount or stamp(doc) != before:
                        raise Rejected("工作文件已移出原挂载或在读取中改变")
                    for above, name, held in opened:
                        check()
                        named = os.open(name, os.O_PATH | os.O_NOFOLLOW | os.O_DIRECTORY | os.O_CLOEXEC, dir_fd=above)
                        try:
                            if not os.path.samestat(os.fstat(named), os.fstat(held)) or mount_id(named) != mount:
                                raise Rejected("工作文件的原路径已变")
                        finally: os.close(named)
                    named = os.open(leaf, os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=parent)
                    try:
                        if _stamp(os.fstat(named)) != before or mount_id(named) != mount:
                            raise Rejected("工作文件的原名称或实际挂载已变")
                    finally: os.close(named)
                    check_relative(parent, leaf, doc, before)

                identical()
                if before[2] != self.facts[1][0]: raise Rejected("工作文件长度已经改变；请先刷新对照")
                yield doc
                identical()

    def compare(self, observer):
        if self.closed: raise Rejected("原逐行对照的句柄已经释放")
        observer.begin("identity")
        self._check_basis()
        self._check_archive()
        observer.begin("members", sum(fact[0] for fact in self.facts))
        raw = _At(self.source, self.observed[2], observer.check)
        with stored_member(raw, raw.end, self.marker, self.member, self.facts[0][0]) as body:
            archived = _verified_small(body.read, self.facts[0], observer)
        with self._working(observer.check) as doc:
            current = _verified_small(doc.read, self.facts[1], observer)
        display = _unified((_small_lines(archived), _small_lines(current)), ("登记基线", "本次工作文件"), observer)
        observer.begin("identity")
        self._check_archive()
        self._check_basis()
        observer.check()
        return display


def request_for_row(pane, selected, page, entry, *, current, confirm_action, indexed=False):
    """One explicit dialog for either archive view. The caller binds its actual row and page."""
    owner = pane.owner
    size = entry.size_bytes
    if indexed and size <= LIMIT: return
    name = f"{page.directory}/{entry.name}" if page.directory else entry.name
    if indexed:
        reading = "\n全文作为严格 UTF-8 检查；核完才可按页码跳读，可能耗时。"
    elif size <= LIMIT:
        reading = "\n显示通过 UTF-8 核验的全文。"
    else:
        reading = "\n仅判断并显示通过核验的开头最多 256 KiB，后续不判定为文本。"
    if (not current() or not confirm_action(pane, "核验后查看纯文本",
            f"归档：{str(selected.path)!r}\n成员：{name!r}\n将从本次原件读取全部 {size:,} 字节。"
            + reading + "\n不打开外部应用、不写文件；可取消。",
            attestation="我确认只查看所选成员，不启动任何程序。", action_text="核验后预览")
            or not current() or owner.pending is not None):
        return
    stopped = Event()

    def cancel():
        stopped.set()
        return True

    def check():
        if stopped.is_set(): raise Rejected("已取消本次预览，未显示或保存正文。")

    def read():
        try: return prepare(selected, page.directory, entry, check=check, indexed=indexed), None
        except Exception as error: return None, error

    def apply(result):
        if not current(): return  # Do not tell a new selection about an old result, even an error.
        preview, error = result
        if stopped.is_set():
            owner.inform("已取消本次预览，未显示或保存正文。")
        elif error is not None:
            owner.inform(str(error))
        else:
            out = indexed_dialog(pane, preview, owner=owner, current=current) if indexed else dialog(pane, preview)
            pane.text_dialog = out
            def finished(_result):
                if pane.text_dialog is out: pane.text_dialog = None
            out.finished.connect(finished)
            out.open()
            owner.inform("本次实读内容与原成员的 SHA-256 相符；未确认来源，没有交给外部应用。")

    owner.job(f"正在核验所选成员的全部 {size:,} 字节…", read, apply,
              cancel=cancel, cancel_text="取消本次文本预览")


class _NotText(ValueError):
    pass


def _carry(data):
    """Defer at most three bytes of a potentially crossing scalar or CRLF."""
    if data.endswith(b"\r"): return 1
    for n in range(1, min(len(data), 3) + 1):
        last = data[-n]
        if 0x80 <= last < 0xc0: continue
        width = (2 if 0xc2 <= last < 0xe0 else 3 if 0xe0 <= last < 0xf0
                 else 4 if 0xf0 <= last <= 0xf4 else 0)
        return n if n < width else 0
    return 0


def _literal(raw):
    s = raw.decode("utf-8").replace("\r\n", "\n")
    if not _showable(s): raise _NotText("含不允许的字符或孤立回车")
    return s


class _Build:
    def __init__(self, size):
        self.size, self.total, self.first = size, 0, ""
        self.blocks, self.carry = bytearray(), b""

    def add(self, raw, end, check):
        i = len(self.blocks) // _BLOCK.size
        left = self.size - i * CHUNK
        if left <= 0 or len(raw) != min(CHUNK, left) or end != i * CHUNK + len(raw):
            raise Rejected("原成员读出的块边界不符")
        combined = self.carry + raw
        back = _carry(combined) if end < self.size else 0
        split = len(combined) - back
        text = _literal(combined[:split])
        self.blocks += _BLOCK.pack(self.total, len(self.carry), hashlib.sha256(raw).digest())
        self.carry = combined[split:]
        for a, b in _page_ranges(text, check):
            if self.total == 0: self.first = text[a:b]
            self.total += 1

    def finish(self, selected, directory, entry, path, start):
        count = (self.size + CHUNK - 1) // CHUNK
        if (self.carry or not self.total or len(self.blocks) != count * _BLOCK.size or
                not 0 <= start <= selected.observed[2] - self.size):
            raise Rejected("无法建立完整的原成员分页凭据")
        return IndexedPreview(selected, directory, entry, path, self.size, start, bytes(self.blocks),
                              self.total, self.first)


@dataclass(frozen=True, slots=True)
class IndexedPreview:
    selected: object
    directory: str
    entry: object
    path: str
    bytes_total: int
    body_at: int
    blocks: bytes
    count: int
    first_page: str

    def _block(self, i):
        return _BLOCK.unpack_from(self.blocks, i * _BLOCK.size)

    def page(self, number, *, check):
        """Display only bytes from verified buffers; never reopen this archive by pathname."""
        if type(number) is not int or not 0 <= number < self.count:
            raise Rejected("页码超出原核验文本的范围")
        if _address(self.selected, self.directory, self.entry) != self.path:
            raise Rejected("已核验原成员不再可用")
        blocks = len(self.blocks) // _BLOCK.size
        i = bisect_right(range(blocks), number, key=lambda n: self._block(n)[0]) - 1
        start, head, _ = self._block(i)
        end, tail = self._block(i + 1)[:2] if i + 1 < blocks else (self.count, 0)

        with _held_input(self.selected, check) as (original, _same_original):
            def checked_block(n):
                if not 0 <= n < blocks: raise Rejected("分页块的位置不符")
                length = min(CHUNK, self.bytes_total - n * CHUNK)
                original.seek(self.body_at + n * CHUNK)
                raw = original.read(length)
                if len(raw) != length or hashlib.sha256(raw).digest() != self._block(n)[2]:
                    raise Rejected("本次读回的文本块与原核验字节不同；未显示该页")
                return raw

            prefix = checked_block(i - 1)[-head:] if head else b""
            block = checked_block(i)
            raw = prefix + block[:len(block) - tail]
            try: text = _literal(raw)
            except (UnicodeError, _NotText) as err:
                raise Rejected("本页不再符合原文件的 UTF-8 分页规则") from err
            shown, pages = None, 0
            for a, b in _page_ranges(text, check):
                if start + pages == number: shown = text[a:b]
                pages += 1
            if shown is None or pages != end - start:
                raise Rejected("本页不再符合原核验的边界")
            return shown


def _viewer(parent, caption):
    """Build one native scrolling, noneditable page; never parse rich text or links."""
    from PySide6.QtCore import Qt
    from PySide6.QtWidgets import QDialog, QHBoxLayout, QPlainTextEdit, QVBoxLayout
    from .widgets import label

    out = QDialog(parent)
    out.setWindowTitle("已核验的纯文本")
    out.setAttribute(Qt.WidgetAttribute.WA_DeleteOnClose)
    out.resize(760, 540)
    box = QVBoxLayout(out)
    box.addWidget(label(caption))
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
    return out, page, text, bar


def dialog(parent, preview):
    from .widgets import button
    description = ("核验后的全文" if preview.bytes_shown == preview.bytes_total
                   else f"只显示前 {preview.bytes_shown:,} 字节，后续内容未作文本判断")
    out, page, text, bar = _viewer(parent, f"{preview.path}\n已核整项 {preview.bytes_total:,} B · {description}")
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


def indexed_dialog(parent, preview, *, owner, current):
    from PySide6.QtWidgets import QLineEdit
    from .widgets import button
    out, page, literal, bar = _viewer(parent, f"{preview.path}\n已核全文 {preview.bytes_total:,} B · 可按页码跳读")
    out.setObjectName("indexed-text-preview")
    proof, count, reading, position = preview, preview.count, None, 0
    jump = QLineEdit()
    jump.setAccessibleName("要跳到的页码")
    jump.setPlaceholderText("页码")
    jump.setMaxLength(len(str(count)))
    jump.setObjectName("indexed-text-page-number")

    def controls(ready):
        back.setEnabled(ready and position > 0)
        more.setEnabled(ready and position + 1 < count)
        jump.setEnabled(ready)
        go.setEnabled(ready)

    def draw(number, value):
        nonlocal position
        position = number
        literal.setPlainText(value)
        literal.verticalScrollBar().setValue(0)
        page.setText(f"{number + 1:,} / {count:,} 页")
        controls(True)

    def ask(number):
        nonlocal reading
        if proof is None or reading is not None or owner.pending is not None or not current():
            return
        if not 0 <= number < count:
            page.setText(f"当前第 {position + 1:,} 页；请输入 1 至 {count:,} 内的页码。")
            return
        original, stop = proof, Event()
        reading = stop
        controls(False)
        literal.clear()
        page.setText(f"正在从原归档核对第 {number + 1:,} 页…")

        def read():
            def check():
                if stop.is_set(): raise Rejected("已关闭文字预览")
            try: return original.page(number, check=check), None
            except Exception as err: return None, str(err)

        def apply(result):
            nonlocal reading
            if reading is not stop: return
            reading = None
            if proof is None or not current():
                out.reject()
                return
            value, failure = result
            if failure is not None:
                out.reject()
                owner.inform(f"分页核对未通过，已关闭预览。请重新选定原件再核验：{failure}")
            else:
                draw(number, value)

        try:
            if owner.job("正在读回并核验本页的原数据块…", read, apply) is None:
                out.reject()
        except Exception as err:
            out.reject()
            owner.inform(f"未能读取本页，预览已关闭：{err}")

    def go_to():
        value = jump.text()
        ask(int(value) - 1 if value.isascii() and value.isdigit() else -1)

    back = button("上一页", lambda: ask(position - 1))
    more = button("下一页", lambda: ask(position + 1))
    go = button("跳到该页", go_to)
    go.setObjectName("indexed-text-go")
    jump.returnPressed.connect(go_to)
    for control in (back, more, jump, go, button("关闭", out.reject)): bar.addWidget(control)

    def clear(_answer):
        nonlocal proof, reading
        stop, reading, proof = reading, None, None
        literal.clear()
        jump.clear()
        if stop is not None: stop.set()

    out.finished.connect(clear)
    draw(0, preview.first_page)
    return out
