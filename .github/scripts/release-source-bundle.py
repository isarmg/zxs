#!/usr/bin/env python3
"""Bind complete Git source, installation instructions and native receipts.

The outer archive is release material, not an executable or a claim that external
Debian/toolchain inputs have been included. Archive paths are never extracted.
"""
from contextlib import contextmanager
import gzip
import hashlib
from pathlib import Path
import runpy
import shutil
import tarfile
import tempfile

SCRIPTS = Path(__file__).resolve().parent
SOURCE = runpy.run_path(str(SCRIPTS / "release-source.py"))
MAX_METADATA_BYTES = 64 * 1024
MAX_BUNDLE_BYTES = SOURCE["MAX_TAR_BYTES"] + 8 * MAX_METADATA_BYTES
MEMBERS = {"BUILD.txt", "VFS-BUILD.json", "INSTALL.md", "repository.tar.gz"}
TEMPLATE = "source/docs/templates/release-install.md"


def sha256(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


@contextmanager
def archive(path, limit):
    with SOURCE["open_archive"](path, limit) as source, gzip.GzipFile(fileobj=source, mode="rb") as compressed:
        reader = SOURCE["BoundedReader"](compressed, limit)
        with tarfile.open(fileobj=reader, mode="r|", ignore_zeros=True) as value:
            yield value
        while reader.read(64 * 1024):
            pass


def instructions(repository, version):
    template = None
    with archive(repository, SOURCE["MAX_TAR_BYTES"]) as value:
        for entry in value:
            if entry.name == TEMPLATE:
                if template is not None or not entry.isfile() or not 0 < entry.size <= MAX_METADATA_BYTES:
                    raise ValueError("invalid source-bound release installation instructions")
                template = value.extractfile(entry).read()
    if template is None:
        raise ValueError("exact source tree lacks release installation instructions")
    result = f"# ZXS {version} 安装\n\n".encode() + template
    if len(result) > MAX_METADATA_BYTES:
        raise ValueError("release installation instructions exceed their byte budget")
    return result


def write_bundle(path, repository, metadata):
    with path.open("xb") as output:
        with gzip.GzipFile(fileobj=output, mode="wb", filename="", mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as value:
                for name, data in sorted(metadata.items()):
                    if not 0 < len(data) <= MAX_METADATA_BYTES:
                        raise ValueError("source bundle metadata exceeds its byte budget")
                    SOURCE["add_bytes"](value, name, 0o644, data)
                entry = tarfile.TarInfo("repository.tar.gz")
                entry.size, entry.mode = repository.stat().st_size, 0o644
                with repository.open("rb") as source:
                    value.addfile(entry, source)


def read_bundle(path, work):
    seen, metadata, total = set(), {}, 0
    with archive(path, MAX_BUNDLE_BYTES) as value:
        for entry in value:
            total += entry.size
            if (entry.name not in MEMBERS or entry.name in seen or not entry.isfile()
                    or entry.mode != 0o644 or entry.uid or entry.gid or entry.uname or entry.gname
                    or entry.mtime or entry.pax_headers or entry.size <= 0 or total > MAX_BUNDLE_BYTES):
                raise ValueError("source bundle must contain exactly the fixed regular members")
            seen.add(entry.name)
            if entry.name == "repository.tar.gz":
                if entry.size > SOURCE["MAX_TAR_BYTES"]:
                    raise ValueError("repository archive exceeds its byte budget")
                with value.extractfile(entry) as source, (work / entry.name).open("xb") as output:
                    shutil.copyfileobj(source, output, length=1024 * 1024)
            else:
                if entry.size > MAX_METADATA_BYTES:
                    raise ValueError("source bundle metadata exceeds its byte budget")
                metadata[entry.name] = value.extractfile(entry).read()
    if seen != MEMBERS:
        raise ValueError("source bundle lacks installation, build or repository material")
    return metadata


def source_identity(repository, fields):
    proof = SOURCE["verify"](repository, fields["commit"], fields["native-web-tree"])
    expected = {"source-archive-commit": fields["commit"],
                "source-archive-tree": fields["native-web-tree"],
                "source-archive-sha256": sha256(repository),
                "source-archive-files": str(proof["file_count"]),
                "source-archive-bytes": str(proof["source_bytes"])}
    if any(fields.get(key) != value for key, value in expected.items()):
        raise ValueError("complete Git source archive differs from the native release receipt")
    return proof


def verify(path, version, commit, binary_hashes):
    assets = runpy.run_path(str(SCRIPTS / "verify-release-assets.py"))
    with tempfile.TemporaryDirectory(prefix="zxs-source-verify-") as temporary:
        work = Path(temporary)
        metadata = read_bundle(path, work)
        fields = assets["build_identity"](metadata["BUILD.txt"], version, commit)
        assets["embedded_vfs_identity"](metadata["VFS-BUILD.json"], fields)
        for name in ("zxs", "zxs-helper"):
            if fields[f"native-{name}-sha256"] != binary_hashes[name]:
                raise ValueError(f"standalone release binary differs from build receipt: {name}")
        repository = work / "repository.tar.gz"
        proof = source_identity(repository, fields)
        if metadata["INSTALL.md"] != instructions(repository, version):
            raise ValueError("release instructions differ from the exact source tree")
        return {"commit": commit, "tree": fields["native-web-tree"], **proof}


def prepare(root, output, version, build, vfs):
    assets = runpy.run_path(str(SCRIPTS / "verify-release-assets.py"))
    fields = assets["build_identity"](build, version)
    assets["embedded_vfs_identity"](vfs, fields)
    if any(key.startswith("source-archive-") for key in fields) or "materials-archive-sha256" in fields:
        raise ValueError("native receipt already contains a packaged archive identity")
    with tempfile.TemporaryDirectory(prefix="zxs-source-package-") as temporary:
        repository = Path(temporary) / "repository.tar.gz"
        source = SOURCE["prepare"](root, repository, fields["commit"], fields["native-web-tree"])
        additions = {f"source-archive-{key}": str(source[key]) for key in ("commit", "tree", "sha256")}
        additions.update({"source-archive-files": str(source["file_count"]),
                          "source-archive-bytes": str(source["source_bytes"])})
        packaged_build = build + "".join(f"{key}={value}\n" for key, value in additions.items()).encode()
        write_bundle(output, repository, {"BUILD.txt": packaged_build, "VFS-BUILD.json": vfs,
                                          "INSTALL.md": instructions(repository, version)})
    return verify(output, version, fields["commit"],
                  {name: fields[f"native-{name}-sha256"] for name in ("zxs", "zxs-helper")})
