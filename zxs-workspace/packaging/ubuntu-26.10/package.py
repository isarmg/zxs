"""Make the local candidate .debs, optionally including the locked HTTPS runtime."""
import argparse
import hashlib
import io
import os
from pathlib import Path
import platform
import re
import stat
import subprocess
import sys
import tomllib
from zipfile import BadZipFile, ZipFile

HERE = Path(__file__).resolve().parent
PROJECT = HERE.parents[1]
BASE = "zip-workspace"
GUI, MOUNT = BASE + "-desktop", BASE + "-mount-ro"
NETWORK = BASE + "-network-r3"
VERSION = "0.1.0~dev0-1local1"
ROOT = "usr/lib/zip-workspace/"
ENTRY = {BASE: "zipworkspace.cli:main", GUI: "zipworkspace.desktop:main", MOUNT: "zipworkspace.linux_mount:main"}
DEPENDS = {
    BASE: "python3 (= 3.14.7-3)",
    GUI: f"{BASE} (= {VERSION}), python3-pyside6.qtcore (= 6.11.2-1build1), "
         "python3-pyside6.qtwidgets (= 6.11.2-1build1), qt6-qpa-plugins (= 6.11.2+dfsg-4ubuntu2), "
         "shared-mime-info (= 2.4-5build3), desktop-file-utils (= 0.28-1build1)",
    NETWORK: f"{GUI} (= {VERSION})",
    MOUNT: f"{BASE} (= {VERSION}), python3-pyfuse3 (= 3.5.0-3), python3-trio (= 0.34.0-1), fuse3 (= 3.18.3-1)",
}
SUMMARY = {BASE: "Local whole-directory ZIP worktrees and command line",
           GUI: "Desktop interface for ZIP Workspace",
           NETWORK: "Optional locked Workspace HTTPS runtime", MOUNT: "Optional read-only FUSE3 snapshot mount"}
PROFILE = "packaging/ubuntu-26.10/"
ASSETS = {"copyright": PROFILE + "COPYRIGHT-NOTICE", "desktop": PROFILE + "zip-workspace.desktop",
          "mime": PROFILE + "zip-workspace-mime.xml"}
INPUT = re.compile(r"([a-f0-9]{64})  (pyproject\.toml|zipworkspace/(?:desktop/)?[A-Za-z0-9_]+\.py|"
                   + "|".join(map(re.escape, ASSETS.values())) + ")")
ENV = {"PATH": "/usr/bin:/bin", "LC_ALL": "C", "TZ": "UTC", "SOURCE_DATE_EPOCH": "1795046400"}


# Exact reviewed universal-wheel closure, not distro Python packages.
NETWORK_LOCK = "93ef643c4e9a10c110fa395ee379128e10fd02ffaa1a4bb7bfdd298a5620231d"
NETWORK_NAMES = ("httpcore", "h11", "h2", "hpack", "hyperframe", "certifi")
LOCK_ROW = re.compile(r"([a-z][a-z0-9]*)(?:\[http2\])?==([0-9.]+) --hash=sha256:([0-9a-f]{64})")
WHEEL_PATH_PART = re.compile(r"[A-Za-z0-9_][A-Za-z0-9_.+-]*")
WHEEL_LIMIT = 2 * 1024 * 1024


def require(condition, message):
    if not condition:
        raise ValueError(message)


def read_inputs(project, lines):
    """Return the verified runtime and installed asset bytes used for this build."""
    inputs = {}
    for line in lines:
        m = INPUT.fullmatch(line)
        require(m, "invalid source manifest line")
        digest, name = m.groups()
        path = project / name
        require(name not in inputs and len(inputs) < 128 and not path.is_symlink()
                and path.is_file() and path.resolve() == path, f"invalid input: {name}")
        with path.open("rb") as src:
            data = src.read(1024 * 1024 + 1)
        require(len(data) <= 1024 * 1024 and hashlib.sha256(data).hexdigest() == digest,
                f"source differs from release profile: {name}")
        inputs[name] = data
    project_meta = tomllib.loads(inputs.pop("pyproject.toml").decode("utf-8"))["project"]
    expected = {**project_meta["scripts"], **project_meta["gui-scripts"]}
    require(project_meta["name"] == BASE and project_meta["version"] == "0.1.0.dev0"
            and expected == ENTRY, "version or entry points differ from this profile")
    for name in ("zipworkspace/cli.py", "zipworkspace/__init__.py", "zipworkspace/desktop/__init__.py",
                 "zipworkspace/linux_mount.py", "zipworkspace/_mount_child.py", "zipworkspace/_fuse3.py"):
        require(name in inputs, f"missing runtime input: {name}")
    assets = {}
    for kind, name in ASSETS.items():
        require(name in inputs, f"missing installed asset: {name}")
        assets[kind] = inputs.pop(name)
    return inputs, assets


def read_network_wheels(wheelhouse, lock):
    """Return the admitted bytes; no package installer, interpreter imports or fetch."""
    require(hashlib.sha256(lock).hexdigest() == NETWORK_LOCK, "network wheel lock differs from this profile")
    require(wheelhouse.is_dir() and wheelhouse.absolute() == wheelhouse.resolve(),
            "use a real local wheel directory")
    runtime = {}
    names = []
    for line in lock.decode("ascii").splitlines():
        if not line or line.startswith("#"):
            continue
        row = LOCK_ROW.fullmatch(line)
        require(row is not None, "invalid network wheel lock row")
        module, version, digest = row.groups()
        require(module in NETWORK_NAMES and module not in names, "unexpected network wheel")
        names.append(module)
        dist_info = f"{module}-{version}.dist-info"
        wheel = wheelhouse / f"{module}-{version}-py3-none-any.whl"
        require(wheel.is_file() and not wheel.is_symlink(), f"missing exact local wheel: {wheel.name}")
        with wheel.open("rb") as f:
            raw = f.read(WHEEL_LIMIT + 1)
        require(len(raw) <= WHEEL_LIMIT and hashlib.sha256(raw).hexdigest() == digest,
                f"wheel differs from lock: {wheel.name}")
        with ZipFile(io.BytesIO(raw)) as archive:
            rows = archive.infolist()
            require(len(rows) <= 2048 and sum(row.file_size for row in rows) <= 16 * WHEEL_LIMIT,
                    f"wheel exceeds package safety bounds: {wheel.name}")
            for item in rows:
                path = item.filename.removesuffix("/")
                parts = path.split("/")
                kind = stat.S_IFMT(item.external_attr >> 16)
                require(item.filename == item.orig_filename
                        and all(WHEEL_PATH_PART.fullmatch(part) and part not in (".", "..") for part in parts)
                        and parts[0] in (module, dist_info),
                        f"unsupported entry in {wheel.name}: {item.filename}")
                if item.is_dir():
                    require(kind in (0, stat.S_IFDIR), f"invalid wheel directory: {path}")
                    continue
                require(len(parts) >= 2 and kind in (0, stat.S_IFREG)
                        and path not in runtime and "__pycache__" not in parts
                        and not path.lower().endswith((".pyc", ".pth", ".so", ".dll", ".dylib", ".pyd")),
                        f"invalid or duplicate wheel entry: {path}")
                runtime[path] = archive.read(item)
            for path in (f"{module}/__init__.py", *(f"{dist_info}/{item}" for item in ("METADATA", "WHEEL", "RECORD"))):
                require(path in runtime, f"missing expected wheel file: {path}")
            require(b"Root-Is-Purelib: true" in runtime[f"{dist_info}/WHEEL"].splitlines()
                    and b"Tag: py3-none-any" in runtime[f"{dist_info}/WHEEL"].splitlines(),
                    f"not an admitted universal pure-Python wheel: {wheel.name}")
    require(tuple(names) == NETWORK_NAMES and bool(runtime.get("certifi/cacert.pem")),
            "missing part of the locked network dependency or CA bundle")
    return runtime


def component(name):
    if name == "zipworkspace/_fuse3.py":
        return MOUNT
    return GUI if name.startswith("zipworkspace/desktop/") else BASE


def launcher(package):
    module, call = ENTRY[package].split(":")
    return (f"#!/usr/bin/python3 -I\nimport sys\nsys.dont_write_bytecode = True\n"
            f"sys.path.insert(0, '/{ROOT.rstrip('/')}')\nfrom {module} import {call}\n"
            f"raise SystemExit({call}())\n").encode()


def put(root, name, data, mode=0o644):
    dest = root / name
    dest.parent.mkdir(parents=True, exist_ok=True)
    with dest.open("xb") as f:
        f.write(data)
    dest.chmod(mode)


def maintainer(value):
    require(value.isascii() and len(value) <= 160
            and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9 .,'-]{0,79} <[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}>", value),
            "supply an authorized ASCII Name <address> for the local package; no control characters")
    return value


def build(out, who, wheelhouse=None):
    who = maintainer(who)
    release = platform.freedesktop_os_release()
    require(os.getuid() != 0 and release.get("ID") == "ubuntu" and release.get("VERSION_ID") == "26.10"
            and release.get("VERSION_CODENAME") == "stonking" and sys.version_info[:2] == (3, 14)
            and subprocess.check_output(["/usr/bin/dpkg", "--print-architecture"], env=ENV).strip() == b"amd64",
            "requires unprivileged Ubuntu 26.10 amd64 with system Python 3.14")
    inputs, assets = read_inputs(PROJECT, (HERE / "INPUTS.sha256").read_text("ascii").splitlines())
    network = None
    if wheelhouse is not None:
        require("zipworkspace/container_https.py" in inputs, "HTTPS client source is not in this release profile")
        lock = (HERE.parent / "network-r3" / "requirements.txt").read_bytes()
        network = read_network_wheels(wheelhouse, lock)
    out = out.absolute()
    require(out == out.resolve() and not out.is_relative_to(PROJECT), "use a new real directory outside the project")
    # The caller owns this new tree and its stable parent. Never reuse or recursively clean it.
    out.mkdir(mode=0o700)
    sums = []
    for pkg in (*ENTRY, NETWORK) if network is not None else ENTRY:
        stage = out / "staging" / pkg
        for name, data in inputs.items():
            if component(name) == pkg:
                put(stage, ROOT + name, data)
        if pkg == NETWORK:
            for name, data in sorted(network.items()):
                put(stage, ROOT + name, data)
            put(stage, f"usr/share/doc/{pkg}/requirements.txt", lock)
        else:
            put(stage, "usr/bin/" + pkg, launcher(pkg), 0o755)
        put(stage, f"usr/share/doc/{pkg}/copyright", assets["copyright"])
        if pkg == GUI:
            put(stage, "usr/share/applications/zip-workspace.desktop", assets["desktop"])
            put(stage, "usr/share/mime/packages/zip-workspace.xml", assets["mime"])
        control = (f"Package: {pkg}\nVersion: {VERSION}\nArchitecture: amd64\nSection: utils\nPriority: optional\n"
                   f"Maintainer: {who}\nDepends: {DEPENDS[pkg]}\nDescription: {SUMMARY[pkg]}\n")
        put(stage, "DEBIAN/control", control.encode("ascii"))
        target = out / f"{pkg}_{VERSION}_amd64.deb"
        subprocess.run(["/usr/bin/dpkg-deb", "--root-owner-group", "-Zxz", "-z6", "--threads-max=1",
                        "--build", str(stage), str(target)], check=True, env=ENV)
        with target.open("rb") as f:
            sums.append(f"{hashlib.file_digest(f, 'sha256').hexdigest()}  {target.name}\n")
    put(out, "SHA256SUMS", "".join(sums).encode("ascii"))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--maintainer", required=True, help="your authorized package contact (written into every .deb)")
    parser.add_argument("--network-wheels", type=Path,
                        help="add the optional HTTPS deb from the six exact local universal wheels")
    args = parser.parse_args()
    os.umask(0o022)  # Package tree permissions must not depend on the operator’s umask.
    try:
        build(args.output, args.maintainer, args.network_wheels)
    except (OSError, ValueError, KeyError, BadZipFile, subprocess.SubprocessError) as e:
        parser.exit(1, f"Package set not complete: {e}\n")
