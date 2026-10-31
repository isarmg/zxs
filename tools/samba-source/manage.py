#!/usr/bin/env python3
"""Verify and materialize the complete, immutable in-repository Samba source."""
import argparse
from contextlib import contextmanager
import hashlib
import json
from pathlib import Path, PurePosixPath
import posixpath
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[2]


def validate_members(members, prefix):
    """Retain upstream internal symlinks, reject escape and special files."""
    for member in members:
        name = PurePosixPath(member.name)
        if name.is_absolute() or ".." in name.parts or not name.parts or name.parts[0] != prefix:
            raise ValueError(f"source path escapes archive root: {member.name}")
        if not (member.isfile() or member.isdir() or member.issym() or member.islnk()):
            raise ValueError(f"unsupported source entry: {member.name}")
        if member.issym() or member.islnk():
            target = member.linkname
            if member.issym():
                target = posixpath.join(posixpath.dirname(member.name), target)
            normalized = PurePosixPath(posixpath.normpath(target))
            if normalized.is_absolute() or not normalized.parts or normalized.parts[0] != prefix:
                raise ValueError(f"source link escapes archive root: {member.name}")


@contextmanager
def assembled_archive(root, receipt):
    """Join unchanged upstream bytes in a private temporary file, never the repo."""
    base = root / "compliance/sources/samba"
    total = hashlib.sha256()
    size = 0
    with tempfile.TemporaryFile() as archive:
        for index, part in enumerate(receipt["parts"]):
            expected_name = f"samba-{receipt['version']}.tar.gz.part-{index:03d}"
            if part["name"] != expected_name:
                raise ValueError("Samba source part order/name mismatch")
            path = base / expected_name
            if not path.resolve().is_relative_to(base.resolve()):
                raise ValueError("Samba source part escapes source directory")
            part_hash = hashlib.sha256()
            part_size = 0
            with path.open("rb") as source:
                while chunk := source.read(1024 * 1024):
                    part_hash.update(chunk)
                    total.update(chunk)
                    archive.write(chunk)
                    part_size += len(chunk)
            if part_size != part["bytes"] or part_hash.hexdigest() != part["sha256"]:
                raise ValueError(f"Samba source part integrity mismatch: {expected_name}")
            size += part_size
        if total.hexdigest() != receipt["sha256"] or size != receipt["archive_bytes"]:
            raise ValueError("Samba source archive integrity mismatch")
        archive.seek(0)
        yield archive


@contextmanager
def verified_source(root=ROOT):
    receipt = json.loads((root / "compliance/sources/samba/source.json").read_text())
    prefix = f"samba-{receipt['version']}"
    with assembled_archive(root, receipt) as archive, tarfile.open(fileobj=archive, mode="r:gz") as source:
        members = source.getmembers()
        validate_members(members, prefix)
        if len(members) != receipt["members"] or sum(m.size for m in members) != receipt["unpacked_file_bytes"]:
            raise ValueError("Samba source archive inventory mismatch")
        for name, expected in receipt["legal_copies"].items():
            content = (root / name).read_bytes()
            upstream = source.extractfile(f"{prefix}/{Path(name).name}").read()
            if content != upstream or hashlib.sha256(content).hexdigest() != expected:
                raise ValueError(f"Samba legal copy mismatch: {name}")
        yield receipt, source


def verify(root=ROOT):
    with verified_source(root) as (receipt, _):
        return receipt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["verify", "extract"])
    parser.add_argument("--destination", type=Path)
    args = parser.parse_args()
    with verified_source() as (receipt, source):
        if args.command == "extract":
            if args.destination is None:
                parser.error("extract requires --destination")
            # Never merge into a stale tree or follow destination symlinks.
            if args.destination.exists() or args.destination.is_symlink():
                raise FileExistsError(args.destination)
            args.destination.parent.mkdir(parents=True, exist_ok=True)
            with tempfile.TemporaryDirectory(prefix=".samba-extract-", dir=args.destination.parent) as temporary:
                staging = Path(temporary) / "source"
                staging.mkdir()
                source.extractall(staging, filter="data")
                staging.rename(args.destination)
    print(json.dumps({"version": receipt["version"], "sha256": receipt["sha256"], "members": receipt["members"]}))


if __name__ == "__main__":
    main()
