"""A fixed inode tree and bounded, verified random reads on one borrowed Store archive."""
from bisect import bisect_left
from dataclasses import dataclass
import errno
import hashlib
import os

from .archive import read_indexed_document, stamp
from .model import CHUNK

ROOT = 1
MAX_READ = 16 << 20


@dataclass(slots=True)
class Inode:
    parent: int
    size: int
    children: list | None       # (UTF-8 component, inode), ordered, only for directories
    nlink: int = 1
    start: int = 0
    hashes: bytes = b""         # one SHA-256 per CHUNK, only for files


class SnapshotView:
    """No storage lookup after admission: an inode denotes the same entry until unmount."""

    def __init__(self, document, *, display_name):
        self.document = document
        self.original = stamp(document)
        self.failed = False
        self.commit, manifest, spans = read_indexed_document(document, display_name=display_name)
        self.total = manifest.total
        self.nodes = {ROOT: Inode(ROOT, 0, [], nlink=2)}
        dirs = {"": ROOT}
        for path in sorted(manifest.rows):
            directory, size, _ = manifest.rows[path]
            parent, _, leaf = path.rpartition("/")
            parent_ino = dirs[parent]
            ino = len(self.nodes) + 1
            if directory:
                dirs[path] = ino
                node = Inode(parent_ino, 0, [], nlink=2)
                self.nodes[parent_ino].nlink += 1
            else:
                start, hashes = spans.pop(path)
                node = Inode(parent_ino, size, None, start=start, hashes=hashes)
            self.nodes[ino] = node
            self.nodes[parent_ino].children.append((leaf.encode("utf-8"), ino))
        self.check()

    def check(self):
        try:
            if self.failed or stamp(self.document) != self.original:
                raise OSError(errno.EIO, "the selected archive is no longer available unchanged")
        except OSError as err:
            self.failed = True
            raise OSError(errno.EIO, "the original snapshot is unavailable") from err

    def node(self, ino):
        try:
            return self.nodes[ino]
        except KeyError:
            raise OSError(errno.ENOENT, "unknown snapshot inode") from None

    def lookup(self, ino, name):
        node = self.node(ino)
        if node.children is None: raise OSError(errno.ENOTDIR, "not a directory")
        if name == b".": return ino
        if name == b"..": return node.parent
        # Lexical ordering of validated Unicode names is the ordering of their UTF-8 encodings.
        i = bisect_left(node.children, (name,))
        if i < len(node.children) and node.children[i][0] == name: return node.children[i][1]
        raise OSError(errno.ENOENT, "no such snapshot member")

    def read(self, ino, offset, size):
        node = self.node(ino)
        if node.children is not None: raise OSError(errno.EISDIR, "not a regular file")
        if offset < 0 or not 0 <= size <= MAX_READ: raise OSError(errno.EINVAL, "unsupported read range")
        self.check()  # Even an empty file or a read at EOF must not revive a failed view.
        end = min(node.size, offset + size)
        result = bytearray()
        try:
            pos = offset
            while pos < end:
                index, within = divmod(pos, CHUNK)
                start = index * CHUNK
                length = min(CHUNK, node.size - start)
                block = bytearray()
                while len(block) < length:
                    try:
                        part = os.pread(self.document.fileno(), length - len(block),
                                        node.start + start + len(block))
                    except InterruptedError:
                        continue
                    if not 0 < len(part) <= length-len(block):
                        raise OSError(errno.EIO, "incomplete block in verified member")
                    block.extend(part)
                if hashlib.sha256(block).digest() != node.hashes[32*index:32*(index+1)]:
                    raise OSError(errno.EIO, "source block differs from the selected snapshot")
                count = min(end - pos, length - within)
                result.extend(memoryview(block)[within:within+count])  # the very bytes just checked
                pos += count
            self.check()
        except OSError as err:
            self.failed = True  # No partial success, even if only the last touched block was wrong.
            raise OSError(errno.EIO, "cannot serve the verified snapshot; unmount and reselect") from err
        return bytes(result)
