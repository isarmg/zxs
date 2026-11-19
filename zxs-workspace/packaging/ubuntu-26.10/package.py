"""Make three local candidate .debs from this profile. Never installs or signs them."""
import argparse
import hashlib
import os
from pathlib import Path
import platform
import re
import subprocess
import sys
import tomllib

HERE = Path(__file__).resolve().parent
PROJECT = HERE.parents[1]
BASE = "zip-workspace"
GUI, MOUNT = BASE + "-desktop", BASE + "-mount-ro"
VERSION = "0.1.0~dev0-1local1"
ROOT = "usr/lib/zip-workspace/"
ENTRY = {BASE: "zipworkspace.cli:main", GUI: "zipworkspace.desktop:main", MOUNT: "zipworkspace.linux_mount:main"}
DEPENDS = {
    BASE: "python3 (= 3.14.7-3)",
    GUI: f"{BASE} (= {VERSION}), python3-pyside6.qtcore (= 6.11.2-1build1), "
         "python3-pyside6.qtwidgets (= 6.11.2-1build1), qt6-qpa-plugins (= 6.11.2+dfsg-4ubuntu2)",
    MOUNT: f"{BASE} (= {VERSION}), python3-pyfuse3 (= 3.5.0-3), python3-trio (= 0.34.0-1), fuse3 (= 3.18.3-1)",
}
SUMMARY = {BASE: "Local whole-directory ZIP worktrees and command line",
           GUI: "Desktop interface for ZIP Workspace", MOUNT: "Optional read-only FUSE3 snapshot mount"}
INPUT = re.compile(r"([a-f0-9]{64})  (pyproject\.toml|zipworkspace/(?:desktop/)?[A-Za-z0-9_]+\.py)")
ENV = {"PATH": "/usr/bin:/bin", "LC_ALL": "C", "TZ": "UTC", "SOURCE_DATE_EPOCH": "1795046400"}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def read_inputs(project, lines):
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
    return inputs


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


def build(out, who):
    who = maintainer(who)
    release = platform.freedesktop_os_release()
    require(os.getuid() != 0 and release.get("ID") == "ubuntu" and release.get("VERSION_ID") == "26.10"
            and release.get("VERSION_CODENAME") == "stonking" and sys.version_info[:2] == (3, 14)
            and subprocess.check_output(["/usr/bin/dpkg", "--print-architecture"], env=ENV).strip() == b"amd64",
            "requires unprivileged Ubuntu 26.10 amd64 with system Python 3.14")
    inputs = read_inputs(PROJECT, (HERE / "INPUTS.sha256").read_text("ascii").splitlines())
    copyright_text = (HERE / "COPYRIGHT-NOTICE").read_bytes()
    desktop = (HERE / "zip-workspace.desktop").read_bytes()
    out = out.absolute()
    require(out == out.resolve() and not out.is_relative_to(PROJECT), "use a new real directory outside the project")
    # The caller owns this new tree and its stable parent. Never reuse or recursively clean it.
    out.mkdir(mode=0o700)
    sums = []
    for pkg in ENTRY:
        stage = out / "staging" / pkg
        for name, data in inputs.items():
            if component(name) == pkg:
                put(stage, ROOT + name, data)
        put(stage, "usr/bin/" + pkg, launcher(pkg), 0o755)
        put(stage, f"usr/share/doc/{pkg}/copyright", copyright_text)
        if pkg == GUI:
            put(stage, "usr/share/applications/zip-workspace.desktop", desktop)
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
    args = parser.parse_args()
    os.umask(0o022)  # Package tree permissions must not depend on the operator’s umask.
    try:
        build(args.output, args.maintainer)
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as e:
        parser.exit(1, f"Package set not complete: {e}\n")
