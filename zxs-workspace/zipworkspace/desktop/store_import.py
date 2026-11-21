"""Admit exactly the displayed successor into the already chosen local STORE."""
import os
import stat

from ..archive import read_managed_document
from ..document import archive_sha, native_identity, relative_document
from ..model import MAX_CD, MAX_TOTAL, Rejected
from ..private_freeze import mount_id
from ..refs import Catalog, STORE_BUDGET
from .member_copy import _AnonymousCopy, _CheckedReads, _generation
from .selection import Selection, StoreSelection


class StoreSuccessor(_AnonymousCopy):
    """A byte-identical import, not a commit, repair, or adoption of a worktree."""

    def __init__(self, selected, store, catalog):
        if not isinstance(selected, Selection) or not isinstance(store, StoreSelection) or not isinstance(catalog, Catalog):
            raise Rejected("需要当前已核快照和同一次所选 STORE 的已核目录")
        mark, anchor = selected.browser.commit, catalog.anchor
        if (str(store.path) != str(catalog.store) or str(anchor.path) != str(selected.path) or
                anchor.marker != mark or anchor.observed != selected.observed or
                selected.exact is not None and selected.exact != anchor or
                not 0 < anchor.observed[2] <= MAX_TOTAL + 2*MAX_CD + (1 << 20)):
            raise Rejected("本次目录与仍持有的原快照不符，不按同名重新选择")
        if mark.operation in catalog.nodes or (mark.branch, mark.generation) in catalog.slots:
            raise Rejected("本库已列本操作或目标代；不覆盖，也不把既有文件当作本次并入")
        prev = catalog.nodes.get(mark.parent)
        if (mark.generation < 2 or prev is None or
                (prev.marker.repo, prev.marker.branch, prev.marker.generation) !=
                (mark.repo, mark.branch, mark.generation - 1) or
                catalog.slots.get((mark.branch, mark.generation - 1)) is not prev or
                catalog.store_gaps.get(mark.parent, mark.parent) is not None or
                catalog.select_branch(mark.branch) is not prev or
                catalog.store_gaps.keys() != catalog.nodes.keys() or any(catalog.store_gaps.values())):
            raise Rejected("只接受原本地历史已核完整、从该分支的确切上一代继续的一个快照")
        self.inventory = {}
        for original in catalog.nodes.values():
            name = original.marker.filename
            if (original.marker.repo != mark.repo or str(original.path) != str(store.path/name)
                    or name in self.inventory):
                raise Rejected("已列历史不是所选 STORE 中互异的原规范槽")
            self.inventory[name] = original.observed
        self.mark, self.raw, self.predecessor, self.store = mark, anchor.sha256, prev, store
        self.parent_archive = None
        super().__init__(selected, str(store.path/mark.filename), anchor.observed[2])

    def _parent(self):
        super()._parent()
        self.store.check()
        current = os.fstat(self.parent)
        if native_identity(current, stat.S_IFDIR) != self.store.identity or stat.S_IMODE(current.st_mode) != 0o700:
            raise Rejected("并入只使用所选本人独用的原 0700 本机 STORE，不改变权限")

    def _actual(self, fd, observed):
        if (_generation(os.fstat(fd)) != observed or
                mount_id(fd, read_proc=self.proc.read) != self.parents[-1][-1]):
            raise Rejected("原 STORE 的规范归档或实际挂载已变，不改选版本")

    def _named(self, name, observed):
        fd = os.open(name, os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=self.parent)
        try: self._actual(fd, observed)
        finally: os.close(fd)

    def _check_predecessor(self, fd):
        self._actual(fd, self.predecessor.observed)
        self._named(self.predecessor.marker.filename, self.predecessor.observed)

    def _inventory(self, installed=None):
        # Reobserve only metadata of the same repo under the held STORE, not
        # the archive contents, another catalog, or any newer branch tip.
        remaining = dict(self.inventory)
        if installed is not None: remaining[self.path.name] = installed
        limit = STORE_BUDGET if installed is not None else STORE_BUDGET - 1  # room for exactly this new name
        cursor = os.open(".", os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=self.parent)
        try:
            with os.scandir(cursor) as entries:
                for index, entry in enumerate(entries, 1):
                    self._checkpoint()
                    if index > limit: raise Rejected("STORE 已达到本次有界目录的上限，不能新增这一槽")
                    name = entry.name
                    if name.startswith(self.mark.repo + ".") and name.endswith(".zxs"):
                        observed = remaining.pop(name, None)
                        if observed is None or _generation(entry.stat(follow_symlinks=False)) != observed:
                            raise Rejected("同仓规范槽已增补或替换；不追新的目录或分支头")
                        self._named(name, observed)
            if remaining: raise Rejected("上次已核的原 STORE 目录不再完整")
        finally: os.close(cursor)

    def _selection_details(self):
        mark, prev = self.mark, self.predecessor
        return (f"仓库：{mark.repo}\n分支：{mark.branch} · 第 {prev.marker.generation} → {mark.generation} 代\n"
                f"实际上一代：{str(prev.path)!r}\n原父节点：{mark.parent} · SHA-256：{prev.sha256}\n"
                f"源 ZIP 原字节 SHA-256：{self.raw}\n"
                "只原样并入这一规范归档，保留节点、版本注释和原字节。"
                "不重建历史、不切换或提交工作树；仅写此独用本机库。\n")

    def review(self):
        try:
            details = super().review()
            self._inventory()
            prev = self.predecessor
            with relative_document(self.parent, prev.marker.filename) as doc:
                self._check_predecessor(doc.fileno())
                reader = _CheckedReads(doc, self._checkpoint, self._bytes_processed)
                self._phase("正在复核原上一代的全部成员")
                actual, _ = read_managed_document(reader, display_name=prev.marker.filename)
                self._phase("正在复核原上一代的完整 ZIP", prev.observed[2])
                if actual != prev.marker or archive_sha(reader) != prev.sha256:
                    raise Rejected("本机 STORE 的原上一代不再符合已核的全包和节点")
                self._check_predecessor(doc.fileno())
                pin = os.dup(doc.fileno())
                self.stack.callback(os.close, pin)
            self.parent_archive = pin
            self._source()
            self._recheck()
            return details
        except BaseException:
            self.close()
            raise

    def _recheck(self, installed=None):
        if self.parent_archive is None: raise Rejected("未持有本次重验的原上一代")
        self._parent()
        self._inventory(installed)
        # Check the true parent last, after the possibly slow bounded inventory.
        self._parent()
        self._check_predecessor(self.parent_archive)

    def _payload(self, create):
        self._recheck()
        return self._whole_snapshot(create, self.raw)

    def _before_link(self):
        # Run this only after all possibly remote source checks. Keep the
        # last prepublication inventory and predecessor tied to local FDs.
        self._recheck()

    def _after_link(self, fd, observed):
        # The base class owns the output FD through this final check. Require
        # its fixed post-link generation, never one sampled from a substitute.
        self._source()
        self._recheck(observed)
        self._named(self.path.name, observed)
        if self._output(fd, 1, self.size) != observed:
            raise Rejected("已出现的原候选与刚发布的文件状态不符")
