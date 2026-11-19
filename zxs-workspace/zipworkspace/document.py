"""Held read-only evidence; no destination or physical publishing capability."""
from __future__ import annotations

from contextlib import contextmanager
import hashlib
import os
from pathlib import Path
import stat

from .archive import stamp
from .model import CHUNK, MAX_CD, MAX_TOTAL, Rejected


def native_identity(meta, kind):
    if (stat.S_IFMT(meta.st_mode) != kind or not meta.st_ino
            or getattr(meta, "st_file_attributes", 0) & stat.FILE_ATTRIBUTE_REPARSE_POINT):
        raise Rejected("cannot prove the selected ordinary object's identity")
    return meta.st_dev, meta.st_ino


@contextmanager
def named_document(path):
    path = Path(path)
    named = native_identity(path.stat(follow_symlinks=False), stat.S_IFREG)
    flags = os.O_RDONLY | getattr(os, "O_BINARY", 0)
    flags |= getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0)
    with os.fdopen(os.open(path, flags), "rb") as doc:
        before = stamp(doc)
        if named != native_identity(os.fstat(doc.fileno()), stat.S_IFREG):
            raise Rejected("selected archive changed before it was opened")
        yield doc
        if before != stamp(doc) or named != native_identity(path.stat(follow_symlinks=False), stat.S_IFREG):
            raise Rejected("selected archive changed during verification")


def archive_sha(doc):
    """Include archive headers/comment; do not chase a growing file."""
    left = os.fstat(doc.fileno()).st_size
    if left > MAX_TOTAL + 2 * MAX_CD + (1 << 20): raise Rejected("archive length outside budget")
    digest = hashlib.sha256()
    doc.seek(0)
    while left:
        data = doc.read(min(left, CHUNK))
        if not data: raise Rejected("selected archive shrank")
        digest.update(data)
        left -= len(data)
    if doc.read(1): raise Rejected("selected archive grew")
    return digest.hexdigest()


def check_relative(parent, name, doc, observed, *, single=False):
    held, named = os.fstat(doc.fileno()), os.stat(name, dir_fd=parent, follow_symlinks=False)
    if (native_identity(held, stat.S_IFREG) != native_identity(named, stat.S_IFREG) or
            stamp(doc) != observed or (named.st_size,named.st_mtime_ns,named.st_ctime_ns) != observed[2:] or
            single and (held.st_nlink != 1 or named.st_nlink != 1)):
        raise Rejected("the original archive leaf was replaced or modified")


@contextmanager
def relative_document(parent, name, *, single=False):
    """Read one existing ordinary archive below a caller-held directory; never follow a link."""
    if not name or name in (".", "..") or Path(name).name != name: raise Rejected("expected one archive name")
    fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC, dir_fd=parent)
    try:
        with os.fdopen(fd, "rb", closefd=False) as doc:
            observed = stamp(doc)
            if observed[2] > MAX_TOTAL + 2*MAX_CD + (1<<20): raise Rejected("archive exceeds the byte budget")
            check_relative(parent, name, doc, observed, single=single)
            yield doc
            check_relative(parent, name, doc, observed, single=single)
    finally: os.close(fd)
