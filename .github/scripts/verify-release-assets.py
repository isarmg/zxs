#!/usr/bin/env python3
"""Verify the complete release set and its source identity before publication."""
import argparse
import hashlib
import os
from pathlib import Path
import re
import shutil
import stat


def open_regular(directory_fd, name):
    # A pre-open lstat is not sufficient: a replaced leaf can become a symlink
    # or FIFO. Anchor every open to the same directory and reject both without
    # following a link, blocking on a pipe, or reading special-device bytes.
    fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=directory_fd)
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or not info.st_size:
            raise ValueError(f"release asset must be a nonempty regular file: {name}")
        return os.fdopen(fd, "rb")
    except BaseException:
        os.close(fd)
        raise


def read_metadata(directory_fd, name):
    with open_regular(directory_fd, name) as source:
        data = source.read(64 * 1024 + 1)
    if len(data) > 64 * 1024:
        raise ValueError(f"release metadata exceeds size limit: {name}")
    return data


def build_identity(build, version, commit=None):
    fields = {}
    for line in build.decode("utf-8").splitlines():
        key, separator, value = line.partition("=")
        if not separator or not key or key in fields:
            raise ValueError("invalid or duplicate release build identity field")
        fields[key] = value
    if fields.get("version") != version or (commit is not None and fields.get("commit") != commit):
        raise ValueError("release build identity differs from the requested version/source")
    if not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", fields.get("commit", "")):
        raise ValueError("invalid native build source identity")
    if fields.get("native-target") != "x86_64-unknown-linux-gnu" or fields.get("native-profile") != "release":
        raise ValueError("native build target/profile identity mismatch")
    if not fields.get("native-rustc"):
        raise ValueError("missing native compiler identity")
    for key in ("native-environment-sha256", "native-inputs-sha256", "native-zxs-sha256", "native-zxs-rescue-sha256"):
        if not re.fullmatch(r"[0-9a-f]{64}", fields.get(key, "")):
            raise ValueError(f"missing or invalid native build identity: {key}")
    return fields


def verify(directory, version, commit, stage=None):
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+([-+][A-Za-z0-9.+-]+)?", version):
        raise ValueError("invalid release version")
    if not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", commit):
        raise ValueError("invalid release source commit")
    directory_fd = os.open(directory, os.O_RDONLY | os.O_NOFOLLOW | os.O_DIRECTORY)
    try:
        return verify_opened(directory_fd, version, commit, stage)
    finally:
        os.close(directory_fd)


def verify_opened(directory_fd, version, commit, stage):
    names = {f"zxs-{version}-x86_64-unknown-linux-gnu.tar.gz",
             f"web-interface-{version}.tar.gz", "INSTALL.md", "BUILD.txt"}
    if set(os.listdir(directory_fd)) != names | {"SHA256SUMS"}:
        raise ValueError("release directory differs from the complete expected asset set")
    receipt = read_metadata(directory_fd, "SHA256SUMS")
    hashes = {}
    for line in receipt.decode("ascii").splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  ([A-Za-z0-9.+_-]+)", line)
        if not match or match[2] in hashes:
            raise ValueError("invalid or duplicate release checksum entry")
        hashes[match[2]] = match[1]
    if set(hashes) != names:
        raise ValueError("release checksum inventory differs from the expected asset set")
    build = read_metadata(directory_fd, "BUILD.txt")
    fields = build_identity(build, version, commit)
    if fields.get("native-archive-sha256") != hashes[f"zxs-{version}-x86_64-unknown-linux-gnu.tar.gz"]:
        raise ValueError("native archive differs from build identity")
    if stage is not None:
        stage = Path(stage)
        for path in (stage, *stage.absolute().parents):
            if path.is_symlink():
                raise ValueError(f"release snapshot symlink: {path}")
        stage.mkdir()
    # Hash the exact stream copied into a private upload snapshot. Subsequent
    # edits to release-assets cannot replace bytes after the validation gate.
    for name, expected in hashes.items():
        with open_regular(directory_fd, name) as source:
            if stage is None:
                actual = hashlib.file_digest(source, "sha256").hexdigest()
            else:
                target = stage / name
                with target.open("xb") as output:
                    shutil.copyfileobj(source, output)
                with target.open("rb") as snapshot:
                    actual = hashlib.file_digest(snapshot, "sha256").hexdigest()
            if actual != expected:
                raise ValueError(f"release checksum mismatch: {name}")
    if hashlib.sha256(build).hexdigest() != hashes["BUILD.txt"]:
        raise ValueError("release build identity changed during verification")
    if stage is not None:
        (stage / "SHA256SUMS").write_bytes(receipt)
    return len(names) + 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, default=Path("release-assets"))
    parser.add_argument("--version", required=True)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--stage", type=Path)
    args = parser.parse_args()
    try:
        count = verify(args.directory, args.version, args.commit, args.stage)
    except (OSError, ValueError) as error:
        parser.exit(1, f"Release asset verification failed: {error}\n")
    print(f"Verified {count} release assets and their source identity.")


if __name__ == "__main__":
    main()
