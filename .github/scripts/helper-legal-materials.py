#!/usr/bin/env python3
"""Snapshot central license/notice bytes from one exact Git tree, never execute them.

This is an inventory and byte binding, not a legal-completeness attestation.
The archive is embedded in the helper, not a new top-level Release asset.
"""
import gzip
import hashlib
import io
import json
from pathlib import Path
import re
import subprocess
import tarfile

FIXED = ("compliance/README.md", "compliance/NOTICE.md",
         "compliance/provenance/origins.json")
LICENSE_ROOT = "compliance/licenses/"
MAX_FILES = 8192
MAX_FILE_BYTES = 16 * 1024 * 1024
MAX_TOTAL_BYTES = 32 * 1024 * 1024
MAX_MANIFEST_BYTES = 4 * 1024 * 1024
ARCHIVE_NAME = "helper-license-materials.tar.gz"
MANIFEST_NAME = "helper-license-materials.json"


def object_id(value):
    if not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", value):
        raise ValueError("material source must be an exact Git object ID")
    return value


def safe_path(value):
    if (not value or len(value.encode("utf-8")) > 512 or "\\" in value
            or any(ord(char) < 32 or ord(char) == 127 for char in value)
            or any(part in ("", ".", "..") for part in value.split("/"))):
        raise ValueError("unsafe material source path")
    if value not in FIXED and not value.startswith(LICENSE_ROOT):
        raise ValueError("material path is outside the fixed central license/notice scope")
    return value


def git(root, *arguments):
    return subprocess.check_output(["git", "-C", str(root), *arguments])


def source_inventory(root, commit, tree):
    object_id(commit)
    object_id(tree)
    if git(root, "rev-parse", f"{commit}^{{tree}}").decode().strip() != tree:
        raise ValueError("material source commit and tree differ")
    raw = git(root, "ls-tree", "-r", "-z", tree, "--", *FIXED, LICENSE_ROOT.rstrip("/"))
    if len(raw) > 4 * 1024 * 1024:
        raise ValueError("material source inventory exceeds its byte budget")
    result = []
    seen = set()
    for record in raw.split(b"\0"):
        if not record:
            continue
        header, separator, path_bytes = record.partition(b"\t")
        if not separator:
            raise ValueError("invalid Git material record")
        fields = header.decode("ascii").split(" ")
        if len(fields) != 3 or fields[0] not in ("100644", "100755") or fields[1] != "blob":
            raise ValueError("material source must contain regular blobs, not links or submodules")
        path = safe_path(path_bytes.decode("utf-8"))
        if path in seen or len(result) == MAX_FILES:
            raise ValueError("duplicate material path or file-count budget exceeded")
        seen.add(path)
        result.append((path, object_id(fields[2])))
    required = {*FIXED, "compliance/licenses/project/LICENSE-APACHE-2.0.txt"}
    if not required.issubset(seen) or not any(path.startswith("compliance/licenses/upstream/") for path in seen):
        raise ValueError("central project/upstream licenses or current notices are missing")
    return sorted(result)


def source_bytes(root, inventory):
    files = []
    total = 0
    for path, blob in inventory:
        size = int(git(root, "cat-file", "-s", blob))
        if size < 0 or size > MAX_FILE_BYTES or total + size > MAX_TOTAL_BYTES:
            raise ValueError("license material source exceeds its bounded file/total size")
        data = git(root, "cat-file", "blob", blob)
        if len(data) != size:
            raise ValueError("material Git blob length differs from its inventory")
        files.append((path, blob, data))
        total += size
    return files


def archive_bytes(manifest, files):
    output = io.BytesIO()
    with gzip.GzipFile(fileobj=output, mode="wb", filename="", mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as archive:
            for name, data in [("MANIFEST.json", manifest), *((path, data) for path, _, data in files)]:
                entry = tarfile.TarInfo(name)
                entry.size = len(data)
                entry.mode = 0o644
                entry.uid = entry.gid = entry.mtime = 0
                entry.uname = entry.gname = ""
                archive.addfile(entry, io.BytesIO(data))
    data = output.getvalue()
    if len(data) > MAX_TOTAL_BYTES:
        raise ValueError("compressed license archive exceeds its embedded byte budget")
    return data


def prepare(root, output, version, commit, tree, payload_sha256):
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+([-+][A-Za-z0-9.+-]+)?", version):
        raise ValueError("invalid material package version")
    if not re.fullmatch(r"[0-9a-f]{64}", payload_sha256):
        raise ValueError("invalid ZXS payload digest for legal materials")
    inventory = source_inventory(root, commit, tree)
    files = source_bytes(root, inventory)
    manifest = {
        "format": "zxs-helper-license-materials", "revision": 1,
        "scope": "central_licenses_notices_and_origin_inventory",
        "version": version, "commit": commit, "tree": tree,
        "target": "x86_64-unknown-linux-gnu", "payload_sha256": payload_sha256,
        "source_url": f"https://github.com/isarmg/zxs/tree/{commit}",
        "legal_completeness_attested": False,
        "open_issues_source": "compliance/README.md",
        "files": [{"path": path, "git_blob": blob, "bytes": len(data),
                   "sha256": hashlib.sha256(data).hexdigest()} for path, blob, data in files],
    }
    manifest_bytes = (json.dumps(manifest, ensure_ascii=False, sort_keys=True, separators=(",", ":")) + "\n").encode()
    if len(manifest_bytes) > MAX_MANIFEST_BYTES:
        raise ValueError("license material manifest exceeds its byte budget")
    archive = archive_bytes(manifest_bytes, files)
    output = Path(output)
    output.mkdir(parents=True, exist_ok=True)
    archive_path, manifest_path = output / ARCHIVE_NAME, output / MANIFEST_NAME
    # These are build outputs only, not runtime paths or Release assets.
    for path, data in ((archive_path, archive), (manifest_path, manifest_bytes)):
        if path.is_symlink():
            raise ValueError("embedded material output must not be a symlink")
        path.write_bytes(data)
    return {"archive": archive_path, "manifest": manifest_path,
            "archive_sha256": hashlib.sha256(archive).hexdigest(),
            "manifest_sha256": hashlib.sha256(manifest_bytes).hexdigest(),
            "file_count": len(files)}
