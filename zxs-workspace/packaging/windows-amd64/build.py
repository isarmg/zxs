"""Build a local unsigned Windows GUI candidate from a prepared offline environment."""
import argparse
import hashlib
from importlib.metadata import PackageNotFoundError, version
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import sysconfig
import tomllib

HERE = Path(__file__).resolve().parent
PROJECT = HERE.parents[1]
PIN = re.compile(r"([A-Za-z0-9_-]+)==([0-9.]+) --hash=sha256:[0-9a-f]{64}")


def require(ok, reason):
    if not ok:
        raise ValueError(reason)


def profile():
    require(sys.platform == "win32" and sys.getwindowsversion().build >= 22000
            and sys.getwindowsversion().product_type == 1
            and sys.implementation.name == "cpython" and sys.version_info[:3] == (3, 14, 10)
            and sys.version_info.releaselevel == "final" and sysconfig.get_platform() == "win-amd64"
            and not sysconfig.get_config_var("Py_GIL_DISABLED"), "Requires Windows 11 x64, official CPython 3.14.10 x64 with GIL")
    require(sys.prefix != sys.base_prefix, "Use a dedicated virtual environment")
    venv = Path(sys.prefix, "pyvenv.cfg").read_text(encoding="utf-8")
    require("include-system-site-packages = false" in venv.lower().splitlines(),
            "Use a fresh venv without system package access")
    pins = {}
    for line in (HERE / "requirements.txt").read_text(encoding="ascii").splitlines():
        line = line.partition("#")[0].strip()
        if not line:
            continue
        match = PIN.fullmatch(line)
        require(match, "Unexpected version-lock syntax")
        name, pin = match.groups()
        require(name not in pins and version(name) == pin, "Wheel version differs from lock: " + name)
        pins[name] = pin
    require(len(pins) == 9, "Incomplete Windows wheel lock")
    with (PROJECT / "pyproject.toml").open("rb") as f:
        meta = tomllib.load(f)["project"]
    require(meta["name"] == "zip-workspace" and
            meta["gui-scripts"] == {"zip-workspace-desktop": "zipworkspace.desktop:main"},
            "Recipe no longer matches the application's GUI entry")
    return meta["version"], pins


def ordinary(path, kind):
    meta = path.lstat()
    require(stat.S_IFMT(meta.st_mode) == kind
            and not (meta.st_file_attributes & stat.FILE_ATTRIBUTE_REPARSE_POINT),
            "Unrecognized output item (links and reparse points are refused): " + str(path))
    return meta


def raise_walk_error(error):
    raise error


def bundle_hashes(bundle):
    ordinary(bundle, stat.S_IFDIR)
    files = {}
    for parent, dirs, names in bundle.walk(on_error=raise_walk_error):
        for name in dirs:
            ordinary(parent / name, stat.S_IFDIR)
        for name in names:
            child = parent / name
            meta = ordinary(child, stat.S_IFREG)
            with child.open("rb") as src:
                digest = hashlib.file_digest(src, "sha256").hexdigest()
            files[child.relative_to(bundle).as_posix()] = {"bytes": meta.st_size, "sha256": digest}
    required = {"ZipWorkspace.exe", "_internal/python314.dll",
                "_internal/PySide6/plugins/platforms/qwindows.dll", "_internal/COPYRIGHT-NOTICE.txt"}
    require(required <= files.keys() and all(files[p]["bytes"] > 0 for p in required),
            "Missing executable, Python, Windows Qt platform plugin or notice")
    names = {Path(k).name.casefold() for k in files}
    require({"qt6core.dll", "qt6gui.dll", "qt6widgets.dll"} <= names
            and not any("winfsp" in n or n.endswith(".sys") for n in names), "Wrong Qt runtime or unexpected driver artifact")
    from pefile import PE
    image = PE(str(bundle / "ZipWorkspace.exe"), fast_load=True)
    try:
        require(image.FILE_HEADER.Machine == 0x8664 and image.OPTIONAL_HEADER.Subsystem == 2,
                "Result is not an x64 Windows GUI executable")
    finally:
        image.close()
    return dict(sorted(files.items()))


def build(out):
    release, wheels = profile()
    # The output parent must be operator-controlled for the entire build, not synced or substituted.
    require(out.is_absolute() and re.fullmatch(r"[A-Za-z]:", out.drive) and out.parent != out
            and out.parent.resolve(strict=True) == out.parent and out.resolve() == out
            and not out.is_relative_to(PROJECT), "Use an absolute new path under a stable private local parent, outside the project")
    ordinary(out.parent, stat.S_IFDIR)
    out.mkdir(mode=0o700)

    tmp = out / "temp"
    tmp.mkdir(mode=0o700)
    empty_upx = out / "empty-upx"
    empty_upx.mkdir(mode=0o700)  # Pin the implicit UPX lookup to an absent executable.
    # Fixed executable and a private tool cache; no ambient Qt, compiler or plugin directories on PATH.
    windows = Path(os.environ["SystemRoot"]).resolve(strict=True)
    env = {n: os.environ[n] for n in ("USERPROFILE", "APPDATA", "LOCALAPPDATA") if n in os.environ}
    env.update(SystemRoot=str(windows), WINDIR=str(windows), COMSPEC=str(windows / "System32" / "cmd.exe"),
               PATH=os.pathsep.join(map(str, (windows / "System32", windows, Path(sys.executable).parent,
                                             Path(sys.base_prefix)))),
               TEMP=str(tmp), TMP=str(tmp), PYINSTALLER_CONFIG_DIR=str(out / "tool-cache"))
    stage = out / "dist"
    subprocess.run([sys.executable, "-I", "-B", "-m", "PyInstaller",
                    "--upx-dir", str(empty_upx), "--workpath", str(out / "work"),
                    "--distpath", str(stage), str(HERE / "ZipWorkspace.spec")],
                   cwd=PROJECT, env=env, check=True)
    ordinary(stage, stat.S_IFDIR)
    require([p.name for p in stage.iterdir()] == ["ZipWorkspace"], "Unexpected candidate output directory")
    record = {"bundle": "dist/ZipWorkspace", "version": release, "platform": "windows-amd64",
              "python": "3.14.10", "build_wheels": wheels, "files": bundle_hashes(stage / "ZipWorkspace")}
    with (out / "candidate.json").open("x", encoding="utf-8") as f:
        json.dump(record, f, indent=2)
        f.write("\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True, help="new absolute directory under your stable private parent")
    try:
        build(parser.parse_args().output)
    except (OSError, ValueError, PackageNotFoundError, subprocess.SubprocessError) as err:
        parser.exit(1, f"No complete candidate: {err}\n")
