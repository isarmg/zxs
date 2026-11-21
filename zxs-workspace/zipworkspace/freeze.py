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


def _scan(rootfd, frozen=None, *, hash_files=False, require_ids=False, check_held=None, checkpoint=None,
          on_frozen=None, on_read=None, before_inventory=None, write_file=None, on_file=None):
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
                if checkpoint is not None: checkpoint()
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
                    if check_held is not None: check_held(path, child)
                    if is_dir:
                        visit(child, path, recheck=recheck)
                    elif not recheck and (frozen is not None or hash_files or write_file is not None):
                        h = hashlib.sha256()
                        count = 0
                        start = frozen.length if frozen is not None else 0
                        target = write_file(path, before.st_size) if write_file is not None else contextlib.nullcontext()
                        with target as packed:
                            while True:
                                if checkpoint is not None: checkpoint()
                                block = os.read(child, CHUNK)
                                if checkpoint is not None: checkpoint()
                                if not block: break
                                count += len(block)
                                if count > before.st_size: raise Rejected("source grew while freezing")
                                h.update(block)
                                if packed is not None and packed.write(block) != len(block):
                                    raise Rejected("short write to the private Store ZIP")
                                if on_read is not None: on_read(len(block))
                                if frozen is not None:
                                    frozen.append(block)
                                    if on_frozen is not None: on_frozen(len(block))
                            if count != before.st_size: raise Rejected("source shrank while freezing")
                        if frozen is not None: frozen.spans[path] = (start,count)
                        manifest.rows[path] = (False, count, h.hexdigest())
                    if _sig(os.fstat(child)) != sig or _sig(os.stat(name, dir_fd=fd, follow_symlinks=False)) != sig:
                        raise Rejected("source or its name changed while freezing")
                    if on_file is not None and hash_files and not recheck and not is_dir:
                        on_file(path, child, before, manifest.rows[path][2])
                finally:
                    os.close(child)
        if _sig(os.fstat(fd)) != _sig(root):
            raise Rejected("directory changed while freezing")
    visit(rootfd, "", recheck=False)
    if before_inventory is not None: before_inventory()
    visit(rootfd, "", recheck=True)
    if facts: raise Rejected("entries missing from the second directory inventory")
    return manifest


def observe_held(fd, expected, *, require_ids=False, check_held=None, checkpoint=None, observer=None, on_file=None):
    """Hash all real descendants of this particular open directory, then recheck the generation."""
    before = os.fstat(fd)
    if (before.st_dev,before.st_ino) != expected or not expected[1] or not stat.S_ISDIR(before.st_mode):
        raise Rejected("not the original held output directory")
    def check():
        if checkpoint is not None: checkpoint()
        observer.check()
    if observer is not None: observer.begin("folder")
    tree = _scan(fd, hash_files=True, require_ids=require_ids, check_held=check_held,
                 checkpoint=checkpoint if observer is None else check,
                 on_read=None if observer is None else observer.advance,
                 before_inventory=None if observer is None else lambda: observer.begin("inventory"),
                 on_file=on_file)
    if _sig(before) != _sig(os.fstat(fd)):
        raise Rejected("selected directory changed during readback")
    if observer is not None: observer.check()
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
def frozen_tree(source, capture, expected, store, *, selected=None, observer=None):
    _native()
    if observer is not None: observer.begin("freeze")
    fd = os.open(source, _dir_flags()) if selected is None else os.dup(selected.work_fd)
    try:
        root_sig = _sig(os.fstat(fd))
        if native_identity(os.fstat(fd),stat.S_IFDIR)!=expected:
            raise Rejected("registered worktree was replaced before capture")
        source_mount = local_mount(fd)
        store_fd = os.open(store,_dir_flags()) if selected is None else os.dup(selected.store_fd)
        try:
            _distinct_roots((fd,capture.parent,store_fd))
            store_id = native_identity(os.fstat(store_fd),stat.S_IFDIR)
            excluded = {capture.identity,store_id}
            def check_held(_, child):
                m = os.fstat(child)
                if not m.st_ino or (m.st_dev,m.st_ino) in excluded or mount_id(child)!=source_mount:
                    raise Rejected("cannot capture aliased state, archive store, or a nested mount")
            capture.check_state()
            tree = _scan(fd, capture, check_held=check_held, checkpoint=None if observer is None else observer.check,
                         on_frozen=None if observer is None else observer.advance)
            if (root_sig!=_sig(os.stat(source,follow_symlinks=False)) or
                    native_identity(os.stat(store,follow_symlinks=False),stat.S_IFDIR)!=store_id):
                raise Rejected("a selected location changed while freezing; no snapshot packed")
            capture.validate(tree)
            capture.check_state()
            if observer is not None: observer.check()
            yield capture,tree
        finally: os.close(store_fd)
    finally: os.close(fd)


def _store_entry(name, directory, size):
    info = zipfile.ZipInfo(name + ("/" if directory else ""), (1980,1,1,0,0,0))
    info.create_system = 3
    info.external_attr = ((0o40755 if directory else 0o100644) << 16) | (16 if directory else 0)
    info.compress_type = zipfile.ZIP_STORED
    info.file_size = size
    return info


def pack_observed_folder(fd, identity, dest, marker_for_tree, *, check_held, checkpoint,
                         on_read=None, before_inventory=None):
    """Build only a private Store ZIP from the same bytes used for the directory marker."""
    before = os.fstat(fd)
    if native_identity(before, stat.S_IFDIR) != identity:
        raise Rejected("not the originally selected folder")
    with zipfile.ZipFile(dest, "w", compression=zipfile.ZIP_STORED, allowZip64=True) as archive:
        contents = _scan(fd, require_ids=True, check_held=check_held, checkpoint=checkpoint,
                         on_read=on_read, before_inventory=before_inventory,
                         write_file=lambda name, size: archive.open(_store_entry(name, False, size), "w"))
        if _sig(before) != _sig(os.fstat(fd)):
            raise Rejected("source folder changed during both observations")
        for name, (directory, _, __) in sorted(contents.rows.items()):
            checkpoint()
            if directory: archive.writestr(_store_entry(name, True, 0), b"")
        marker = marker_for_tree(contents.digest())
        archive.comment = marker.encode()
    checkpoint()
    return marker, contents.total


def pack_chosen_folder(fd, identity, dest, repo, branch, operation, **checks):
    return pack_observed_folder(fd, identity, dest, lambda tree: Commit(repo, branch, 1, operation, None, tree),
                                **checks)


def pack(frozen, tree: Manifest, marker: Commit, destination, *, observer=None):
    if observer is not None: observer.begin("pack", tree.total)
    if marker.tree != tree.digest(): raise Rejected("wrong frozen generation")
    frozen.validate(tree)
    with zipfile.ZipFile(destination, "w", compression=zipfile.ZIP_STORED, allowZip64=True) as archive:
        for name, (directory,size,digest) in sorted(tree.rows.items()):
            if observer is not None: observer.check()
            info = _store_entry(name, directory, size)
            if directory:
                archive.writestr(info,b"")
            else:
                h = hashlib.sha256()
                with archive.open(info,"w") as out:
                    for block in frozen.blocks(name,size):
                        if observer is not None: observer.check()
                        h.update(block)
                        if out.write(block) != len(block): raise Rejected("short write while packing the private member")
                        if observer is not None: observer.advance(len(block))
                if h.hexdigest()!=digest: raise Rejected("frozen bytes no longer match the captured file")
        archive.comment = marker.encode()
    if observer is not None: observer.begin("sync")
    destination.flush(); os.fsync(destination.fileno())
    if observer is not None: observer.check()
