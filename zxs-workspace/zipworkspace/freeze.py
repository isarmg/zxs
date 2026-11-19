"""Read a selected tree; fresh commits package a complete anonymous frozen copy."""
from __future__ import annotations
import contextlib
import hashlib
import os
from pathlib import Path
import stat
import zipfile
from .document import native_identity
from .model import CHUNK, Commit, Manifest, Rejected
from .private_freeze import local_mount, mount_id

def _sig(m):
    return m.st_dev, m.st_ino, m.st_mode, m.st_size, m.st_mtime_ns, m.st_ctime_ns


def _native():
    if os.name != "posix" or not hasattr(os, "O_NOFOLLOW") or os.open not in os.supports_dir_fd:
        raise Rejected("secure checkout capture is not yet available on this platform")


def _dir_flags():
    return os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC


def _scan(rootfd, frozen=None, *, hash_files=False, require_ids=False, check_held=None):
    facts = {}
    manifest = Manifest()
    rootdev = os.fstat(rootfd).st_dev
    def visit(fd, prefix, *, recheck):
        root = os.fstat(fd)
        if root.st_dev != rootdev:
            raise Rejected("nested mount is not part of this portable worktree")
        # Iterate streaming. Budgets take effect before retaining a full directory.
        with os.scandir(fd) as entries:
            for entry in entries:
                path = f"{prefix}/{entry.name}" if prefix else entry.name
                before = entry.stat(follow_symlinks=False)
                if require_ids and not before.st_ino:
                    raise Rejected("cannot verify a member's native identity")
                is_dir = stat.S_ISDIR(before.st_mode)
                if not is_dir and not stat.S_ISREG(before.st_mode):
                    raise Rejected("checkout contains a link or non-regular entry")
                if before.st_dev != rootdev:
                    raise Rejected("checkout member is on another device")
                sig = _sig(before)
                if recheck:
                    # Only an exact previously admitted name and generation can
                    # pass. A duplicate also has no remaining first-pass entry.
                    if facts.pop(path, None) != sig:
                        raise Rejected("entry changed between directory inventories")
                else:
                    manifest.add(path, is_dir, 0 if is_dir else before.st_size, None)
                    facts[path] = sig
                name = entry.name
                flags = _dir_flags() if is_dir else os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC
                child = os.open(name, flags, dir_fd=fd)
                try:
                    if _sig(os.fstat(child)) != sig:
                        raise Rejected("checkout changed during admission")
                    if check_held is not None: check_held(child)
                    if is_dir:
                        visit(child, path, recheck=recheck)
                    elif not recheck and (frozen is not None or hash_files):
                        h = hashlib.sha256()
                        count = 0
                        start = frozen.length if frozen is not None else 0
                        while block := os.read(child, CHUNK):
                            count += len(block)
                            if count > before.st_size: raise Rejected("source grew while freezing")
                            h.update(block)
                            if frozen is not None: frozen.append(block)
                        if count != before.st_size: raise Rejected("source shrank while freezing")
                        if frozen is not None: frozen.spans[path] = (start,count)
                        manifest.rows[path] = (False, count, h.hexdigest())
                    if _sig(os.fstat(child)) != sig or _sig(os.stat(name, dir_fd=fd, follow_symlinks=False)) != sig:
                        raise Rejected("source or its name changed while freezing")
                finally:
                    os.close(child)
        if _sig(os.fstat(fd)) != _sig(root):
            raise Rejected("directory changed while freezing")
    visit(rootfd, "", recheck=False)
    visit(rootfd, "", recheck=True)
    if facts: raise Rejected("entries missing from the second directory inventory")
    return manifest


def observe_held(fd, expected, *, require_ids=False):
    """Hash all real descendants of this particular open directory, then recheck the generation."""
    before = os.fstat(fd)
    if (before.st_dev,before.st_ino) != expected or not expected[1] or not stat.S_ISDIR(before.st_mode):
        raise Rejected("not the original held output directory")
    tree = _scan(fd, hash_files=True, require_ids=require_ids)
    if _sig(before) != _sig(os.fstat(fd)):
        raise Rejected("selected directory changed during readback")
    return tree


def observe_tree(source: Path):
    """Read back actual named files without making a second private data copy."""
    _native()
    fd = os.open(source, _dir_flags())
    try:
        root_sig = _sig(os.fstat(fd))
        tree = _scan(fd, hash_files=True)
        if root_sig != _sig(os.stat(source, follow_symlinks=False)):
            raise Rejected("checkout changed during readback")
        return tree
    finally: os.close(fd)


def _distinct_roots(fds):
    roots = [native_identity(os.fstat(fd),stat.S_IFDIR) for fd in fds]
    if len(set(roots)) != len(roots): raise Rejected("working tree, private state and store must be different")
    for start,root in zip(fds,roots):
        current = os.dup(start)
        try:
            for _ in range(256):
                parent = os.open("..",_dir_flags(),dir_fd=current)
                try:
                    here = native_identity(os.fstat(current),stat.S_IFDIR)
                    above = native_identity(os.fstat(parent),stat.S_IFDIR)
                    if above in roots and above != root:
                        raise Rejected("one selected directory contains another")
                    if above==here: break
                    os.close(current); current = parent; parent = None
                finally:
                    if parent is not None: os.close(parent)
            else: raise Rejected("cannot establish independent selected directory roots")
        finally: os.close(current)


@contextlib.contextmanager
def frozen_tree(source, capture, expected, store):
    _native()
    fd = os.open(source, _dir_flags())
    try:
        root_sig = _sig(os.fstat(fd))
        if native_identity(os.fstat(fd),stat.S_IFDIR)!=expected:
            raise Rejected("registered worktree was replaced before capture")
        source_mount = local_mount(fd)
        store_fd = os.open(store,_dir_flags())
        try:
            _distinct_roots((fd,capture.parent,store_fd))
            store_id = native_identity(os.fstat(store_fd),stat.S_IFDIR)
            excluded = {capture.identity,store_id}
            def check_held(child):
                m = os.fstat(child)
                if not m.st_ino or (m.st_dev,m.st_ino) in excluded or mount_id(child)!=source_mount:
                    raise Rejected("cannot capture aliased state, archive store, or a nested mount")
            capture.check_state()
            tree = _scan(fd,capture,check_held=check_held)
            if (root_sig!=_sig(os.stat(source,follow_symlinks=False)) or
                    native_identity(os.stat(store,follow_symlinks=False),stat.S_IFDIR)!=store_id):
                raise Rejected("a selected location changed while freezing; no snapshot packed")
            capture.validate(tree)
            capture.check_state()
            yield capture,tree
        finally: os.close(store_fd)
    finally: os.close(fd)


def pack(frozen, tree: Manifest, marker: Commit, destination):
    if marker.tree != tree.digest(): raise Rejected("wrong frozen generation")
    frozen.validate(tree)
    with zipfile.ZipFile(destination, "w", compression=zipfile.ZIP_STORED, allowZip64=True) as archive:
        for name, (directory,size,digest) in sorted(tree.rows.items()):
            info = zipfile.ZipInfo(name + ("/" if directory else ""), (1980,1,1,0,0,0))
            info.create_system = 3
            info.external_attr = ((0o40755 if directory else 0o100644) << 16) | (16 if directory else 0)
            info.compress_type = zipfile.ZIP_STORED
            info.file_size = size
            if directory:
                archive.writestr(info,b"")
            else:
                h = hashlib.sha256()
                with archive.open(info,"w") as out:
                    for block in frozen.blocks(name,size): h.update(block); out.write(block)
                if h.hexdigest()!=digest: raise Rejected("frozen bytes no longer match the captured file")
        archive.comment = marker.encode()
    destination.flush(); os.fsync(destination.fileno())
