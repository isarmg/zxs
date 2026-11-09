#!/usr/bin/env python3
"""Verify the complete release set and its source identity before publication."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import runpy
import tempfile
import tarfile
import stat
import struct


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
    for key in ("native-environment-sha256", "native-inputs-sha256", "native-zxs-sha256", "native-zxs-helper-sha256"):
        if not re.fullmatch(r"[0-9a-f]{64}", fields.get(key, "")):
            raise ValueError(f"missing or invalid native build identity: {key}")
    if (fields.get("native-helper-embedded-zxs") != "true"
            or fields.get("native-helper-payload-sha256") != fields["native-zxs-sha256"]):
        raise ValueError("helper must embed the exact standalone ZXS build payload")
    if fields.get("native-helper-embedded-vfs") != "true":
        raise ValueError("helper must embed the matching offline VFS product")
    for key in ("native-helper-vfs-payload-sha256", "native-helper-vfs-build-sha256"):
        if not re.fullmatch(r"[0-9a-f]{64}", fields.get(key, "")):
            raise ValueError("helper VFS byte identities are missing or invalid")
    if (fields.get("native-helper-license-materials-embedded") != "true"
            or fields.get("native-helper-license-commit") != fields["commit"]
            or fields.get("native-helper-license-tree") != fields.get("native-web-tree")
            or not re.fullmatch(r"[1-9][0-9]{0,3}", fields.get("native-helper-license-file-count", ""))
            or int(fields["native-helper-license-file-count"]) > 8192):
        raise ValueError("helper legal materials must bind the exact source and bounded file inventory")
    for key in ("native-helper-license-materials-sha256", "native-helper-license-manifest-sha256"):
        if not re.fullmatch(r"[0-9a-f]{64}", fields.get(key, "")):
            raise ValueError("helper legal material byte identities are missing or invalid")
    if fields.get("native-baseline") != "debian13-amd64" or fields.get("native-web-embedded") != "true":
        raise ValueError("native release requires Debian 13 and embedded Web")
    if fields.get("native-web-commit") != fields["commit"] or not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", fields.get("native-web-tree", "")):
        raise ValueError("embedded Web source identity mismatch")
    if not re.fullmatch(r"[0-9a-f]{64}", fields.get("native-web-manifest-sha256", "")):
        raise ValueError("missing embedded Web manifest identity")
    runtime_text = fields.get("native-runtime-dependencies", "")
    if hashlib.sha256(runtime_text.encode()).hexdigest() != fields.get("native-runtime-dependencies-sha256"):
        raise ValueError("native runtime dependency receipt hash mismatch")
    runtime = json.loads(runtime_text)
    if (runtime.get("schema_version") != 1 or runtime.get("distribution") != "debian"
            or runtime.get("release") != "13" or runtime.get("architecture") != "amd64"):
        raise ValueError("native runtime dependency receipt target mismatch")
    binaries = runtime.get("binaries", [])
    if len(binaries) != 2 or {binary.get("name") for binary in binaries} != {"zxs", "zxs-helper"}:
        raise ValueError("native runtime dependency binary inventory mismatch")
    for binary in binaries:
        if binary.get("sha256") != fields[f"native-{binary['name']}-sha256"] or not binary.get("libraries"):
            raise ValueError("native runtime dependency binary identity mismatch")
        for library in binary["libraries"]:
            if (not library.get("package") or not library.get("version")
                    or not library.get("path", "").startswith("/")
                    or not library.get("resolved_path", "").startswith("/")
                    or not re.fullmatch(r"[0-9a-f]{64}", library.get("sha256", ""))):
                raise ValueError("native runtime dependency library identity incomplete")
    return fields


def elf_identity(header, module=False):
    # This identifies the declared ELF ABI/type, not executable correctness.
    # Runtime HTTP/browser and real Samba loading are separate required gates.
    if (len(header) < 64 or header[:7] != b"\x7fELF\x02\x01\x01"
            or struct.unpack_from("<H", header, 16)[0] not in ((3,) if module else (2, 3))
            or struct.unpack_from("<H", header, 18)[0] != 62
            or struct.unpack_from("<I", header, 20)[0] != 1
            or struct.unpack_from("<H", header, 52)[0] != 64):
        raise ValueError("release product must be an ELF64 little-endian x86_64 executable/shared object")


def asset_names(version):
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+([-+][A-Za-z0-9.+-]+)?", version):
        raise ValueError("invalid release version")
    return {f"zxs-{version}-debian13-amd64", f"zxs-helper-{version}-debian13-amd64",
            f"zxs-source-{version}.tar.gz"}


def vfs_identity(data, fields):
    receipt = json.loads(data)
    if (receipt.get("commit") != fields["commit"] or receipt.get("tree") != fields["native-web-tree"]
            or receipt.get("distribution") != "debian" or receipt.get("release") != "13"
            or receipt.get("architecture") != "amd64" or receipt.get("vfs_interface") != 50
            or receipt.get("package_version") != "2:4.22.11+dfsg-0+deb13u1"
            or not re.fullmatch(r"[0-9a-f]{64}", receipt.get("module_sha256", ""))):
        raise ValueError("VFS build identity differs from the matched Debian source/runtime")
    return receipt


def embedded_vfs_identity(data, fields):
    receipt = vfs_identity(data, fields)
    if (fields["native-helper-vfs-payload-sha256"] != receipt["module_sha256"]
            or fields["native-helper-vfs-build-sha256"] != hashlib.sha256(data).hexdigest()):
        raise ValueError("embedded helper VFS differs from its same-source build receipt")
    return receipt


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
    names = asset_names(version)
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
    bundle = runpy.run_path(str(Path(__file__).with_name("release-source-bundle.py")))
    limits = {name: bundle["MAX_BUNDLE_BYTES"] if name.endswith(".tar.gz") else 512 * 1024 * 1024
              for name in names}
    # Validate and copy the same held input streams. A private complete snapshot
    # is required even for verify-only, because nested archive checks must never
    # reopen mutable release paths after their checksums have been verified.
    with tempfile.TemporaryDirectory(prefix="zxs-release-verify-") as temporary:
        snapshot = Path(temporary)
        for name, expected in hashes.items():
            size, digest = 0, hashlib.sha256()
            with open_regular(directory_fd, name) as source, (snapshot / name).open("xb") as output:
                first = source.read(1024 * 1024)
                if not name.endswith(".tar.gz"):
                    elf_identity(first[:64])
                chunk = first
                while chunk:
                    size += len(chunk)
                    if size > limits[name]:
                        raise ValueError(f"release asset exceeds its byte budget: {name}")
                    digest.update(chunk)
                    output.write(chunk)
                    chunk = source.read(1024 * 1024)
            if digest.hexdigest() != expected:
                raise ValueError(f"release checksum mismatch: {name}")
        bundle["verify"](snapshot / f"zxs-source-{version}.tar.gz", version, commit,
                         {name: hashes[f"{name}-{version}-debian13-amd64"] for name in ("zxs", "zxs-helper")})
        if stage is not None:
            stage = Path(stage)
            for path in (stage, *stage.absolute().parents):
                if path.is_symlink():
                    raise ValueError(f"release snapshot symlink: {path}")
            stage.mkdir()
            for name in names:
                with (snapshot / name).open("rb") as source, (stage / name).open("xb") as output:
                    while chunk := source.read(1024 * 1024):
                        output.write(chunk)
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
    except (OSError, ValueError, tarfile.TarError, EOFError) as error:
        parser.exit(1, f"Release asset verification failed: {error}\n")
    print(f"Verified {count} release assets and their source identity.")


if __name__ == "__main__":
    main()

