"""Compare ordinary unnamed file streams on an admitted, held local Windows volume."""
import hashlib
import os
from pathlib import Path
import re

from .comparison import FolderEvidence
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


def walk(api, drive, parts):
    """Only the drive anchor is opened by absolute spelling. Everything else is handle-relative."""
    current = api.root(drive, not parts)
    try:
        volume, original = api.info(current.value)
        if not original.directory: raise Rejected("selected drive anchor is not a directory")
        guid = api.admit_volume(current.value)
        for n, part in enumerate(parts):
            child = api.child(current.value, part, "directory" if n+1 == len(parts) else "ancestor")
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


def observe(api, root, volume, first):
    tree, facts = Manifest(), {}

    def held(h, expected):
        v, found = api.info(h)
        if v != volume or found != expected:
            raise Rejected("opened Windows entry no longer matches the original inventory")

    def visit(parent, prefix, directory, checking, seen, directories):
        held(parent, directory)
        dots = set()
        for name, row in api.listing(parent):
            if name in DOT:
                if name in dots: raise Rejected("nonprogressing Windows directory listing")
                dots.add(name)
                continue
            path = prefix + "/" + name if prefix else name
            if checking:
                if facts.get(path) != row or path in seen:
                    raise Rejected("directory inventory, spelling, or generation changed")
                seen.add(path)
            else:
                tree.add(path, row.directory, row.size, None)
                facts[path] = row
            if row.directory:
                if row.file_id in directories: raise Rejected("repeated directory identity in selected folder")
                directories.add(row.file_id)
            kind = "directory" if row.directory else "file"
            with api.child(parent, name, kind) as child:
                held(child, row)
                if row.directory:
                    visit(child, path, row, checking, seen, directories)
                elif not checking:
                    left, digest = row.size, hashlib.sha256()
                    while left:
                        block = api.read(child, min(CHUNK,left))
                        if not 0 < len(block) <= min(CHUNK,left):
                            raise Rejected("size of the held Windows file changed")
                        left -= len(block)
                        digest.update(block)
                    if api.read(child,1):
                        raise Rejected("extra bytes in the held Windows file")
                    tree.rows[path] = (False,row.size,digest.hexdigest())
                held(child, row)
                # A valid held file alone does not prove its original directory entry.
                with api.child(parent,name,kind) as named:
                    held(named,row)
        held(parent,directory)

    visit(root,"",first,False,None,{first.file_id})
    second = set()
    visit(root,"",first,True,second,{first.file_id})
    if len(second) != len(facts):
        raise Rejected("a Windows directory entry disappeared during comparison")
    return tree


class WindowsFolder:
    """Single job owns the live picker handle. Only evidence is retained by the result."""
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
        if self.handle.value is None: raise Rejected("the chosen Windows folder handle is closed")
        v, current = self.api.info(self.handle.value)
        if v != self.volume or not current.directory or current.file_id != self.file_id:
            raise Rejected("the held Windows folder no longer has its selected identity")
        named, guid, nv, native = walk(self.api,self.drive,self.parts)
        with named:
            if guid != self.guid or nv != v or native != current:
                raise Rejected("the chosen Windows name no longer identifies this held folder")
        return self.handle.value

    def observe(self):
        h = self.check()
        v, initial = self.api.info(h)
        if v != self.volume or initial.file_id != self.file_id or not initial.directory:
            raise Rejected("the selected Windows directory changed before reading")
        tree = observe(self.api,h,v,initial)
        self.check()
        return tree

    def close(self):
        self.handle.close()
