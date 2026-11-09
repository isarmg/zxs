"""The only network input accepted by the temporary SQLite maintenance job."""
from dataclasses import dataclass
import hashlib
import io
import json
from pathlib import PurePosixPath
import re
import tarfile
import tomllib
from urllib.request import HTTPRedirectHandler, Request, build_opener

NAME = "libsqlite3-sys"
VERSION = "0.39.1"
IDENTITY = NAME + "-" + VERSION
URL = "https://static.crates.io/crates/libsqlite3-sys/libsqlite3-sys-0.39.1.crate"
SHA256 = "14a5f01b87b92705d07b433c06f96cbc764637d81a0bffb5734ce17cf0f9a2b3"
# rust-lang/crates.io-index@e01f438da0564e13f027e5d06b5e2688e05e13cf, li/bs/libsqlite3-sys
MAX_ARCHIVE = 8 * 1024 * 1024
MAX_EXPANDED = 64 * 1024 * 1024
MAX_FILE = 32 * 1024 * 1024
MAX_ENTRIES = 512


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def relative(path):
    if (not isinstance(path, str) or not path or "\\" in path
            or any(p in ("", ".", "..", ".git") or ":" in p for p in path.split("/"))
            or PurePosixPath(path).is_absolute()):
        raise ValueError("unsafe archive/source path: " + repr(path))
    return path


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, new_url):
        raise ValueError("the fixed crate URL must not redirect")


@dataclass(frozen=True)
class Release:
    archive: bytes
    files: dict
    manifest: dict
    vcs: dict

    @property
    def inventory(self):
        return {name: sha256(data) for name, data in sorted(self.files.items())}


def download():
    request = Request(URL, headers={"Accept-Encoding": "identity"})
    with build_opener(NoRedirect).open(request, timeout=30) as response:
        if response.status != 200 or response.geturl() != URL:
            raise ValueError("unexpected crate response")
        data = response.read(MAX_ARCHIVE + 1)
    if len(data) > MAX_ARCHIVE or sha256(data) != SHA256:
        raise ValueError("crate size or fixed SHA-256 mismatch")
    return admit(data)


def admit(archive):
    # Hash admission precedes parsing. Never run an upstream build/import script.
    if len(archive) > MAX_ARCHIVE or sha256(archive) != SHA256:
        raise ValueError("unauthenticated SQLite archive")
    files, names, total = {}, set(), 0
    with tarfile.open(fileobj=io.BytesIO(archive), mode="r|gz") as source:
        for number, member in enumerate(source, 1):
            if number > MAX_ENTRIES:
                raise ValueError("too many archive entries")
            name = member.name.rstrip("/") if member.isdir() else member.name
            if not name.startswith(IDENTITY + "/"):
                raise ValueError("foreign archive root")
            path = relative(name[len(IDENTITY) + 1:])
            if path in names or not (member.isfile() or member.isdir()):
                raise ValueError("duplicate or non-regular archive member")
            names.add(path)
            if member.isdir():
                continue
            if member.size < 0 or member.size > MAX_FILE:
                raise ValueError("oversized archive member")
            total += member.size
            if total > MAX_EXPANDED:
                raise ValueError("expanded archive exceeds budget")
            stream = source.extractfile(member)
            if stream is None:
                raise ValueError("missing archive member bytes")
            data = stream.read(MAX_FILE + 1)
            if len(data) != member.size:
                raise ValueError("truncated archive member")
            # This particular source release must be text; no executable/binary drop.
            data.decode("utf-8")
            files[path] = data
    required = {"Cargo.toml", "Cargo.toml.orig", ".cargo_vcs_info.json", "LICENSE",
                "README.md", "src/lib.rs", "src/error.rs", "build.rs", "wrapper.h", "wrapper_ext.h",
                "sqlite3/sqlite3.c", "sqlite3/sqlite3.h", "sqlite3/sqlite3ext.h",
                "sqlite3/bindgen_bundled_version.rs"}
    if not required <= files.keys() or not any(p.startswith("bindgen-bindings/") for p in files):
        raise ValueError("incomplete upstream source, bindings or legal materials")
    manifest = tomllib.loads(files["Cargo.toml"].decode("utf-8"))
    original = tomllib.loads(files["Cargo.toml.orig"].decode("utf-8"))
    for document in (manifest, original):
        package = document["package"]
        if (package["name"] != NAME or package["version"] != VERSION
                or package["license"] != "MIT"
                or package["repository"] != "https://github.com/rusqlite/rusqlite"):
            raise ValueError("archive package/version/license/repository mismatch")
    if b"Permission is hereby granted" not in files["LICENSE"]:
        raise ValueError("missing MIT grant")
    vcs = json.loads(files[".cargo_vcs_info.json"])
    if (not re.fullmatch(r"[0-9a-f]{40}", vcs["git"]["sha1"])
            or vcs["git"].get("dirty", False) is not False
            or vcs.get("path_in_vcs") != "libsqlite3-sys"):
        raise ValueError("incomplete upstream VCS provenance")
    source_ids = []
    for name in ("sqlite3/sqlite3.c", "sqlite3/sqlite3.h"):
        text = files[name].decode("utf-8")
        if (re.findall(r'^#define SQLITE_VERSION[ \t]+"([^"]+)"', text, re.M) != ["3.54.1"]
                or re.findall(r"^#define SQLITE_VERSION_NUMBER[ \t]+([0-9]+)", text, re.M) != ["3054001"]):
            raise ValueError("amalgamation/header is not SQLite 3.54.1")
        ids = re.findall(r'^#define SQLITE_SOURCE_ID[ \t]+"([^"]+)"', text, re.M)
        if len(ids) != 1:
            raise ValueError("missing SQLite source identity")
        source_ids.append(ids[0])
    if source_ids[0] != source_ids[1]:
        raise ValueError("SQLite C/header source identity differs")
    bindings = files["sqlite3/bindgen_bundled_version.rs"].decode("utf-8")
    if re.findall(r"pub const SQLITE_VERSION_NUMBER: [^=]+=[ \t]*([0-9]+);", bindings) != ["3054001"]:
        raise ValueError("bundled bindings do not match SQLite 3.54.1")
    return Release(archive, files, manifest, vcs)
