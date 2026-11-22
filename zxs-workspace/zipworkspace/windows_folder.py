"""Compare ordinary unnamed file streams on an admitted, held local Windows volume."""
from contextlib import ExitStack, contextmanager
import hashlib
import os
from pathlib import Path
import re
from threading import Lock

from .comparison import FolderEvidence, _check_held_archive, _small_changed_left
from .model import CHUNK, MAX_DEPTH, Manifest, Rejected, member_name
from .windows_nt import DOT


def drive_path(path):
    path = os.fspath(path)
    if not isinstance(path, str) or len(path) > 32767:
        raise Rejected("select an ordinary absolute Windows folder")
    path = path.replace("/", "\\")
    if not re.match(r"^[A-Za-z]:\\", path):
        raise Rejected("use an absolute drive-letter folder, not UNC, device, or relative names")
    tail = path[3:]
    if tail.endswith("\\"):
        if not tail[:-1] or tail.endswith("\\\\"):
            raise Rejected("ambiguous directory separators")
        tail = tail[:-1]
    parts = tuple(tail.split("\\")) if tail else ()
    if len(parts) > MAX_DEPTH or any(len(member_name(p)) != 1 for p in parts):
        raise Rejected("unsupported selected directory components")
    return path[0].upper(), parts


def walk(api, drive, parts, *, list_last=True):
    """Only the drive anchor is opened by absolute spelling. Everything else is handle-relative."""
    current = api.root(drive, list_last and not parts)
    try:
        volume, original = api.info(current.value)
        if not original.directory: raise Rejected("selected drive anchor is not a directory")
        guid = api.admit_volume(current.value)
        for n, part in enumerate(parts):
            child = api.child(current.value, part, "directory" if list_last and n+1 == len(parts) else "ancestor")
            previous, current = current, child
            previous.close()
            v, original = api.info(current.value)
            if v != volume or not original.directory:
                raise Rejected("selected path crossed a volume or ceased to be a directory")
        if api.guid_name(current.value) != guid + "\\".join(parts):
            raise Rejected("selected folder no longer has its exact native long path")
        return current, guid, volume, original
    except BaseException:
        current.close()
        raise


def observe(api, root, volume, first, *, observer=None, on_file=None):
    tree, facts = Manifest(), {}

    def from_listing(h, listed):
        # NTFS can delay copying metadata to the directory index. Use the
        # enumerated identity/type to select the child, and the handle for its generation.
        v, found = api.info(h)
        if v != volume or found.file_id != listed.file_id or found.directory != listed.directory:
            raise Rejected("opened Windows entry is not the inventoried object")
        return found

    def held(h, expected):
        v, found = api.info(h)
        if v != volume or found != expected:
            raise Rejected("opened Windows entry no longer matches its original held observation")

    def visit(parent, prefix, directory, checking, seen, directories):
        if observer is not None: observer.check()
        held(parent, directory)
        dots = set()
        for name, listed in api.listing(parent):
            if observer is not None: observer.check()
            if name in DOT:
                if name in dots: raise Rejected("nonprogressing Windows directory listing")
                dots.add(name)
                continue
            path = prefix + "/" + name if prefix else name
            if checking and (path not in facts or path in seen):
                raise Rejected("directory inventory or spelling changed")
            kind = "directory" if listed.directory else "file"
            with api.child(parent, name, kind) as child:
                original = from_listing(child, listed)
                if checking:
                    if original != facts[path]:
                        raise Rejected("Windows entry generation changed between the two observations")
                    seen.add(path)
                else:
                    tree.add(path, original.directory, original.size, None)
                    facts[path] = original
                if original.directory:
                    if original.file_id in directories:
                        raise Rejected("repeated directory identity in selected folder")
                    directories.add(original.file_id)
                    visit(child, path, original, checking, seen, directories)
                elif not checking:
                    left, digest = original.size, hashlib.sha256()
                    while left:
                        if observer is not None: observer.check()
                        block = api.read(child, min(CHUNK,left))
                        if observer is not None: observer.check()
                        if not 0 < len(block) <= min(CHUNK,left):
                            raise Rejected("size of the held Windows file changed")
                        left -= len(block)
                        digest.update(block)
                        if observer is not None: observer.advance(len(block))
                    if observer is not None: observer.check()
                    if api.read(child,1):
                        raise Rejected("extra bytes in the held Windows file")
                    tree.rows[path] = (False,original.size,digest.hexdigest())
                held(child, original)
                # A valid held file alone does not prove its original directory entry.
                with api.child(parent,name,kind) as named:
                    held(named, original)
                if on_file is not None and not checking and not original.directory:
                    on_file(path, child, original, tree.rows[path][2])
        held(parent,directory)

    if observer is not None: observer.begin("folder")
    visit(root,"",first,False,None,{first.file_id})
    if observer is not None: observer.begin("inventory")
    second = set()
    visit(root,"",first,True,second,{first.file_id})
    if len(second) != len(facts):
        raise Rejected("a Windows directory entry disappeared during comparison")
    if observer is not None: observer.check()
    return tree


class WindowsFolder:
    """Single job owns the original picker handle; a report may retain a same-access duplicate."""
    def __init__(self, api, drive, parts, handle, guid, volume, first):
        self.api, self.drive, self.parts, self.handle = api, drive, parts, handle
        self.guid, self.volume, self.file_id = guid, volume, first.file_id
        self.evidence = FolderEvidence(Path(drive+":\\"+"\\".join(parts)),
                                       (volume,int.from_bytes(self.file_id,"little")))

    @classmethod
    def choose(cls, path, api):
        if api is None: raise Rejected("native Windows folder reader unavailable")
        drive, parts = drive_path(path)
        h, guid, v, first = walk(api,drive,parts)
        try:
            result = cls(api,drive,parts,h,guid,v,first)
            result.check()
            return result
        except BaseException:
            h.close()
            raise

    def check(self):
        return self._check_handle(self.handle.value)

    def _check_handle(self, opened):
        if opened is None: raise Rejected("the chosen Windows folder handle is closed")
        v, current = self.api.info(opened)
        if v != self.volume or not current.directory or current.file_id != self.file_id:
            raise Rejected("the held Windows folder no longer has its selected identity")
        named, guid, nv, native = walk(self.api,self.drive,self.parts)
        with named:
            if guid != self.guid or nv != v or native != current:
                raise Rejected("the chosen Windows name no longer identifies this held folder")
        return opened

    def observe(self, *, observer=None, on_file=None):
        h = self.check()
        v, initial = self.api.info(h)
        if v != self.volume or initial.file_id != self.file_id or not initial.directory:
            raise Rejected("the selected Windows directory changed before reading")
        options = {} if observer is None else {"observer": observer}
        tree = observe(self.api,h,v,initial,on_file=on_file,**options)
        self.check()
        return tree

    def close(self):
        self.handle.close()


class WindowsFolderText:
    """One original report's small-file witnesses and the same native read-only directory."""
    _held = None

    def __init__(self, selected, folder, original):
        self.selected, self.folder, self.api = selected, folder, folder.api
        self._archive = self._archive_state(original)
        self._lock, self._pairs = Lock(), {}

    def _archive_state(self, fd):
        import msvcrt

        _check_held_archive(self.selected, fd)
        drive, parts = drive_path(self.selected.path)
        if not parts: raise Rejected("原归档没有准确的长文件名")
        h = msvcrt.get_osfhandle(fd)  # Borrowed; its existing CRT owner closes it.
        parent, guid, volume, _ = walk(self.api, drive, parts[:-1], list_last=False)
        with parent as original_parent, self.api.child(original_parent, parts[-1], "file-info") as named:
            name = guid + "\\".join(parts)
            body_volume, row, links = self.api.file_state(h)
            if (body_volume != volume or row.directory or links != 1 or row.size != self.selected.observed[2] or
                    self.api.file_state(named) != (volume, row, 1) or
                    self.api.guid_name(h) != name or self.api.guid_name(named) != name):
                raise Rejected("不能确认原归档是固定 NTFS 的普通单链接文件及其准确长名")
            _check_held_archive(self.selected, fd)
        return name, volume, row

    def _check_archive(self, original):
        if self._archive_state(original) != self._archive:
            raise Rejected("原快照的实际卷、长名或文件身份改变；未打开替件")

    def _name(self, parts):
        return self.folder.guid + "\\".join(self.folder.parts + parts)

    def _check_root(self, root):
        self.folder._check_handle(root)
        if self.api.guid_name(root) != self._name(()):
            raise Rejected("原文件夹的实际长名已经改变")

    def _check_file(self, item, parts, expected):
        if (self.api.file_state(item) != (self.folder.volume, expected, 1) or
                self.api.guid_name(item) != self._name(parts)):
            raise Rejected("本机文件已不是本次报告中的原 NTFS 文件及长名；请刷新对照")

    def remember(self, path, item, expected, sha):
        previous = _small_changed_left(self.selected, path, expected.size, sha)
        if previous is None: return
        try: self._check_file(item, member_name(path), expected)
        except (OSError, Rejected): return  # Keep the ordinary list, never grant text from an unproved row.
        self._pairs[path] = expected, previous, sha

    def bind(self, rows, original):
        self._check_archive(original)
        self._pairs = {row.path: (row, *self._pairs[row.path]) for row in rows if row.path in self._pairs}
        if not self._pairs: return False
        with ExitStack() as guard:
            duplicate = self.api.duplicate(self.folder.check())
            guard.callback(duplicate.close)
            self._check_root(duplicate.value)
            self._held = duplicate
            guard.pop_all()
        return True

    def _pair(self, selected, row):
        if self._held is None or self.selected is not selected or row is None: return None
        pair = self._pairs.get(row.path)
        return pair if pair is not None and pair[0] is row else None

    def supports(self, selected, row):
        with self._lock: return self._pair(selected, row) is not None

    def close(self):
        if self._held is None: return
        with self._lock:
            retained, self._held = self._held, None
            self._pairs.clear()
        if retained is not None: retained.close()

    __del__ = close

    def _relative(self, root, parts, expected, scope, check, *, names_only=False):
        current = root
        for i, part in enumerate(parts[:-1], 1):
            check()
            current = scope.enter_context(self.api.child(current, part, "ancestor"))
            volume, row = self.api.info(current)
            if volume != self.folder.volume or not row.directory or self.api.guid_name(current) != self._name(parts[:i]):
                raise Rejected("原文件父目录不在同一 NTFS 卷或准确长名已变")
        check()
        item = scope.enter_context(self.api.child(current, parts[-1], "file-info" if names_only else "file"))
        self._check_file(item, parts, expected)
        return item

    @contextmanager
    def open_pair(self, selected, row, check):
        check()
        with ExitStack() as held:
            with self._lock:
                pair = self._pair(selected, row)
                if pair is None: raise Rejected("不属于本次文件夹对照的原差异行")
                root = held.enter_context(self.api.duplicate(self._held.value))
            _, expected, left_sha, right_sha = pair
            parts = member_name(row.path)
            if expected.size != row.after_bytes: raise Rejected("原文件与报告中的大小不符")
            original = held.enter_context(selected.held.read_lease())

            def originals():
                check()
                self._check_archive(original)
                self._check_root(root)
                check()

            originals()
            item = self._relative(root, parts, expected, held, check)
            yield original, lambda n: self.api.read(item, n), left_sha, right_sha
            originals()
            self._check_file(item, parts, expected)
            with ExitStack() as names:
                self._relative(root, parts, expected, names, check, names_only=True)
            check()
