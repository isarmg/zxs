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
