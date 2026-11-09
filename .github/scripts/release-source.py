#!/usr/bin/env python3
"""Archive exact Git source objects, independently of checkout/export filters.

This is repository-source identity, not an external dependency completeness or
reproducible-build attestation. It never extracts or executes archived files.
"""
import argparse
import gzip
import hashlib
import io
import os
from pathlib import Path
import re
import stat
import subprocess
import tarfile

MAX_FILES = 65536
MAX_FILE_BYTES = 64 * 1024 * 1024
MAX_TOTAL_BYTES = 512 * 1024 * 1024
MAX_COMMIT_BYTES = 64 * 1024
MAX_INVENTORY_BYTES = 16 * 1024 * 1024
MAX_TAR_BYTES = MAX_TOTAL_BYTES + MAX_FILES * 2048 + MAX_COMMIT_BYTES


def object_id(value):
    if not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", value):
        raise ValueError("source identity must be an exact Git object ID")
    return value


def object_hash(kind, data, identity):
    digest = hashlib.new("sha1" if len(identity) == 40 else "sha256")
    digest.update(kind.encode() + b" " + str(len(data)).encode() + b"\0" + data)
    return digest.hexdigest()


def source_path(value):
    parts = value.split("/")
    if (not value or len(value.encode()) > 512 or "\\" in value or ":" in value
            or any(ord(char) < 32 or ord(char) == 127 for char in value)
            or any(part in ("", ".", "..") or part.lower() == ".git" for part in parts)):
        raise ValueError("unsafe source archive path")
    return value


def git(root, *arguments, limit=MAX_COMMIT_BYTES):
    command = ["git", "--no-replace-objects", "-C", str(root), *arguments]
    with subprocess.Popen(command, stdout=subprocess.PIPE) as process:
        data = process.stdout.read(limit + 1)
        if len(data) > limit:
            process.kill()
            raise ValueError("Git source metadata exceeds its byte budget")
        status = process.wait()
        if status != 0:
            raise subprocess.CalledProcessError(status, command)
    return data


def commit_identity(data, commit, tree):
    object_id(commit)
    object_id(tree)
    if (len(data) > MAX_COMMIT_BYTES or object_hash("commit", data, commit) != commit
            or data.split(b"\n", 1)[0] != b"tree " + tree.encode()):
        raise ValueError("source commit bytes do not bind the requested commit/tree")


def inventory(root, commit, tree):
    object_id(commit)
    object_id(tree)
    size = int(git(root, "cat-file", "-s", commit))
    if size > MAX_COMMIT_BYTES:
        raise ValueError("source commit exceeds its byte budget")
    raw_commit = git(root, "cat-file", "commit", commit)
    commit_identity(raw_commit, commit, tree)
    raw = git(root, "ls-tree", "-r", "-l", "-z", tree, limit=MAX_INVENTORY_BYTES)
    files, seen, total = [], set(), 0
    for record in raw.split(b"\0"):
        if not record:
            continue
        header, separator, path = record.partition(b"\t")
        fields = header.split()
        if (not separator or len(fields) != 4 or fields[0] not in (b"100644", b"100755")
                or fields[1] != b"blob"):
            raise ValueError("source must contain regular Git blobs, not links or submodules")
        name, blob, size = source_path(path.decode()), object_id(fields[2].decode()), int(fields[3])
        total += size
        if (name in seen or len(files) == MAX_FILES or not 0 <= size <= MAX_FILE_BYTES
                or total > MAX_TOTAL_BYTES):
            raise ValueError("duplicate source path or source byte/count budget exceeded")
        seen.add(name)
        files.append((name, int(fields[0], 8), blob, size))
    if not files:
        raise ValueError("source tree is empty")
    return raw_commit, sorted(files), total


def tree_identity(files, identity):
    root = {}
    for path, mode, blob in files:
        parts, folder = source_path(path).split("/"), root
        for part in parts[:-1]:
            folder = folder.setdefault(part, {})
            if not isinstance(folder, dict):
                raise ValueError("source file/directory collision")
        if parts[-1] in folder:
            raise ValueError("duplicate source path or file/directory collision")
        folder[parts[-1]] = (mode, blob)

    def encode(folder):
        body = bytearray()
        ordered = sorted(folder.items(), key=lambda item: item[0].encode() +
                         (b"/" if isinstance(item[1], dict) else b""))
        for name, entry in ordered:
            mode, blob = (0o40000, encode(entry)) if isinstance(entry, dict) else entry
            body.extend(f"{mode:o} ".encode() + name.encode() + b"\0" + bytes.fromhex(blob))
        return object_hash("tree", body, identity)

    return encode(root)


def add_bytes(archive, name, mode, data):
    entry = tarfile.TarInfo(name)
    entry.size, entry.mode = len(data), mode
    entry.uid = entry.gid = entry.mtime = 0
    archive.addfile(entry, io.BytesIO(data))


class BoundedReader:
    def __init__(self, source, limit):
        self.source, self.remaining = source, limit

    def read(self, size=-1):
        if size < 0 or size > self.remaining:
            size = self.remaining + 1
        data = self.source.read(size)
        self.remaining -= len(data)
        if self.remaining < 0:
            raise ValueError("source archive exceeds its uncompressed byte budget")
        return data


def open_archive(path, limit=MAX_TAR_BYTES):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or not 0 < info.st_size <= limit:
            raise ValueError("source archive must be a bounded nonempty regular file")
        return os.fdopen(fd, 'rb')
    except BaseException:
        os.close(fd)
        raise


def verify(path, commit, tree):
    object_id(commit)
    object_id(tree)
    files, total, seen = [], 0, set()
    with open_archive(path) as source, gzip.GzipFile(fileobj=source, mode="rb") as compressed:
        bounded = BoundedReader(compressed, MAX_TAR_BYTES)
        with tarfile.open(fileobj=bounded, mode="r|", ignore_zeros=True) as archive:
            for entry in archive:
                if (not entry.isfile() or entry.name in seen or entry.uid or entry.gid
                        or entry.uname or entry.gname or entry.mtime
                        or entry.mode not in (0o644, 0o755) or set(entry.pax_headers) - {'path'}):
                    raise ValueError("noncanonical, duplicate or nonregular source archive entry")
                seen.add(entry.name)
                if entry.name == "SOURCE-COMMIT":
                    if entry.size > MAX_COMMIT_BYTES or entry.mode != 0o644:
                        raise ValueError("source commit exceeds its byte budget")
                    commit_identity(archive.extractfile(entry).read(), commit, tree)
                    continue
                if not entry.name.startswith("source/"):
                    raise ValueError("source archive entry outside its fixed scope")
                name = source_path(entry.name.removeprefix("source/"))
                total += entry.size
                if (len(files) == MAX_FILES or not 0 <= entry.size <= MAX_FILE_BYTES
                        or total > MAX_TOTAL_BYTES):
                    raise ValueError("source archive exceeds its file/total budget")
                data = archive.extractfile(entry).read()
                files.append((name, entry.mode | 0o100000, object_hash("blob", data, tree)))
        # Also force the gzip trailer/CRC and bounded trailing bytes to be read.
        while bounded.read(64 * 1024):
            pass
    if "SOURCE-COMMIT" not in seen or not files or tree_identity(files, tree) != tree:
        raise ValueError("source archive does not contain the complete requested Git tree")
    return {"file_count": len(files), "source_bytes": total}


def prepare(root, output, commit, tree):
    raw_commit, files, total = inventory(root, commit, tree)
    output = Path(output)
    # The caller provides a new private build-stage path, not a host install path.
    with output.open("xb") as destination:
        try:
            with gzip.GzipFile(fileobj=destination, mode="wb", filename="", mtime=0) as compressed:
                with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as archive:
                    add_bytes(archive, "SOURCE-COMMIT", 0o644, raw_commit)
                    with subprocess.Popen(["git", "--no-replace-objects", "-C", str(root),
                                           "cat-file", "--batch"], stdin=subprocess.PIPE,
                                          stdout=subprocess.PIPE) as process:
                        for name, mode, blob, size in files:
                            process.stdin.write(blob.encode() + b"\n")
                            process.stdin.flush()
                            header = process.stdout.readline(256)
                            if header != f"{blob} blob {size}\n".encode():
                                raise ValueError("Git source blob identity changed")
                            data = process.stdout.read(size)
                            if (len(data) != size or process.stdout.read(1) != b"\n"
                                    or object_hash("blob", data, blob) != blob):
                                raise ValueError("Git source blob bytes differ from inventory")
                            add_bytes(archive, "source/" + name, mode & 0o777, data)
                        process.stdin.close()
                        if process.wait() != 0:
                            raise ValueError("Git source object reader failed")
            destination.flush()
            verified = verify(output, commit, tree)
            if verified != {"file_count": len(files), "source_bytes": total}:
                raise ValueError("source archive inventory changed while writing")
        except BaseException as error:
            try:
                output.unlink()
            except OSError as cleanup:
                error.add_note(f"Could not remove incomplete source archive {output}: {cleanup}")
            raise
    with output.open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    return {"commit": commit, "tree": tree, "sha256": digest, **verified}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--tree", required=True)
    args = parser.parse_args()
    try:
        result = verify(args.archive, args.commit, args.tree)
    except (OSError, ValueError, tarfile.TarError, EOFError) as error:
        parser.exit(1, f"Source archive verification failed: {error}\n")
    print(f"Verified {result['file_count']} Git source files ({result['source_bytes']} bytes); no extraction.")


if __name__ == "__main__":
    main()
