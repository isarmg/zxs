"""POSIX source capture. Freeze a private copy first, and build ZIP only from it."""
from __future__ import annotations
import contextlib
import hashlib
import os
from pathlib import Path
import stat
import tempfile
import zipfile
from .model import CHUNK, Commit, Manifest, Rejected

def _sig(m):
    return m.st_dev, m.st_ino, m.st_mode, m.st_size, m.st_mtime_ns, m.st_ctime_ns


def _native():
    if os.name != "posix" or not hasattr(os, "O_NOFOLLOW") or os.open not in os.supports_dir_fd:
        raise Rejected("secure checkout capture is not yet available on this platform")


def _dir_flags():
    return os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC


def _scan(rootfd, frozen=None, *, hash_files=False):
    facts = {}
    manifest = Manifest()
    rootdev = os.fstat(rootfd).st_dev
    def visit(fd, prefix):
        root = os.fstat(fd)
        if root.st_dev != rootdev:
            raise Rejected("nested mount is not part of this portable worktree")
        # Iterate streaming. Budgets take effect before retaining a full directory.
        with os.scandir(fd) as entries:
            for entry in entries:
                path = f"{prefix}/{entry.name}" if prefix else entry.name
                before = entry.stat(follow_symlinks=False)
                is_dir = stat.S_ISDIR(before.st_mode)
                if not is_dir and not stat.S_ISREG(before.st_mode):
                    raise Rejected("checkout contains a link or non-regular entry")
                if before.st_dev != rootdev:
                    raise Rejected("checkout member is on another device")
                manifest.add(path, is_dir, 0 if is_dir else before.st_size, None)
                facts[path] = _sig(before)
                name = entry.name
                flags = _dir_flags() if is_dir else os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC
                child = os.open(name, flags, dir_fd=fd)
                try:
                    if _sig(os.fstat(child)) != facts[path]:
                        raise Rejected("checkout changed during admission")
                    if is_dir:
                        if frozen is not None: (frozen / path).mkdir(mode=0o700)
                        visit(child, path)
                    elif frozen is not None or hash_files:
                        h = hashlib.sha256()
                        count = 0
                        output = (frozen / path).open("xb") if frozen is not None else contextlib.nullcontext(None)
                        with output as out:
                            while block := os.read(child, CHUNK):
                                count += len(block)
                                if count > before.st_size: raise Rejected("source grew while freezing")
                                h.update(block)
                                if out is not None: out.write(block)
                        if count != before.st_size: raise Rejected("source shrank while freezing")
                        manifest.rows[path] = (False, count, h.hexdigest())
                    if _sig(os.fstat(child)) != facts[path] or _sig(os.stat(name, dir_fd=fd, follow_symlinks=False)) != facts[path]:
                        raise Rejected("source or its name changed while freezing")
                finally:
                    os.close(child)
        if _sig(os.fstat(fd)) != _sig(root):
            raise Rejected("directory changed while freezing")
    visit(rootfd, "")
    return facts, manifest


def observe_tree(source: Path):
    """Read back actual named files without making a second private data copy."""
    _native()
    fd = os.open(source, _dir_flags())
    try:
        root_sig = _sig(os.fstat(fd))
        facts, tree = _scan(fd, hash_files=True)
        now, _ = _scan(fd)
        if facts != now or root_sig != _sig(os.stat(source, follow_symlinks=False)):
            raise Rejected("checkout changed during readback")
        return tree
    finally: os.close(fd)


@contextlib.contextmanager
def frozen_tree(source: Path, private_parent: Path):
    _native()
    if private_parent.resolve().is_relative_to(source.resolve()):
        raise Rejected("put all application state and temporary files outside the worktree")
    fd = os.open(source, _dir_flags())
    try:
        root_sig = _sig(os.fstat(fd))
        with tempfile.TemporaryDirectory(prefix="freeze-", dir=private_parent) as temp:
            frozen = Path(temp)
            facts, tree = _scan(fd, frozen)
            now, _ = _scan(fd)
            if facts != now or root_sig != _sig(os.stat(source, follow_symlinks=False)):
                raise Rejected("checkout generation changed; no archive was packaged")
            # An unmounted/quiescent checkout was copied; the archive consumer is
            # handed only this private generation, never a path into the live tree.
            yield frozen, tree
    finally:
        os.close(fd)


def pack(frozen: Path, tree: Manifest, marker: Commit, destination: Path):
    if marker.tree != tree.digest(): raise Rejected("wrong frozen generation")
    with destination.open("xb") as raw:
        with zipfile.ZipFile(raw, "w", compression=zipfile.ZIP_STORED, allowZip64=True) as archive:
            for name, (directory, size, digest) in sorted(tree.rows.items()):
                info = zipfile.ZipInfo(name + ("/" if directory else ""), (1980, 1, 1, 0, 0, 0))
                info.create_system = 3
                info.external_attr = ((0o40755 if directory else 0o100644) << 16) | (16 if directory else 0)
                info.compress_type = zipfile.ZIP_STORED
                info.file_size = size
                if directory:
                    archive.writestr(info, b"")
                else:
                    h = hashlib.sha256()
                    with (frozen / name).open("rb") as src, archive.open(info, "w") as out:
                        while data := src.read(CHUNK):
                            h.update(data); out.write(data)
                    if h.hexdigest() != digest: raise Rejected("frozen member no longer matches")
            archive.comment = marker.encode()
        raw.flush()
        os.fsync(raw.fileno())
