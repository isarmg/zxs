"""Build one local Apple Silicon GUI archive, not a Developer ID release."""
import argparse
import hashlib
from importlib.metadata import PackageNotFoundError, version
import json
import os
from pathlib import Path
import platform
import plistlib
import re
import stat
import subprocess
import sys
import sysconfig
import tomllib

HERE = Path(__file__).resolve().parent
PROJECT = HERE.parents[1]
PIN = re.compile(r"([A-Za-z0-9_-]+)==([0-9.]+) --hash=sha256:[0-9a-f]{64}")
RELEASE = re.compile(r"([0-9]+\.[0-9]+\.[0-9]+)(?:\.dev[0-9]+)?")


def require(ok, reason):
    if not ok:
        raise ValueError(reason)


def profile():
    require(sys.platform == "darwin" and platform.machine() == "arm64" and os.getuid() != 0
            and os.getuid() == os.geteuid() and platform.mac_ver()[0].split(".", 1)[0] == "26"
            and sys.implementation.name == "cpython" and sys.version_info[:3] == (3, 14, 10)
            and sys.version_info.releaselevel == "final" and not sysconfig.get_config_var("Py_GIL_DISABLED"),
            "Use an ordinary account on native Apple Silicon macOS 26 with official CPython 3.14.10 with GIL")
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
    require(len(pins) == 8, "Incomplete macOS wheel lock")
    with (PROJECT / "pyproject.toml").open("rb") as f:
        meta = tomllib.load(f)["project"]
    release = RELEASE.fullmatch(meta["version"])
    require(release and meta["name"] == "zip-workspace" and
            meta["gui-scripts"] == {"zip-workspace-desktop": "zipworkspace.desktop:main"},
            "Recipe no longer matches the app version or GUI entry")
    return release.group(), release.group(1), pins


def raise_walk_error(error):
    raise error


def verify_app(app, marketing, env):
    require(app.is_dir() and app.resolve(strict=True) == app, "Missing real .app bundle")
    file_names = set()
    for directory, dirs, files in app.walk(on_error=raise_walk_error):
        for leaf in dirs + files:
            path = directory / leaf
            mode = path.lstat().st_mode
            if stat.S_ISLNK(mode):
                require(not path.readlink().is_absolute() and path.resolve(strict=True).is_relative_to(app),
                        "Bundle link is not relative and self-contained: " + str(path))
            elif stat.S_ISREG(mode):
                file_names.add(leaf)
            else:
                require(stat.S_ISDIR(mode), "Unexpected special file in app: " + str(path))
    require({"Python", "QtCore", "QtGui", "QtWidgets", "COPYRIGHT-NOTICE.txt"} <= file_names
            and (app / "Contents/Frameworks/PySide6/Qt/plugins/platforms/libqcocoa.dylib").is_file(),
            "Missing Python, Qt, Cocoa platform plugin or provenance notice")
    require(not any("macfuse" in n.casefold() or "winfsp" in n.casefold() for n in file_names),
            "Unexpected filesystem provider")
    with (app / "Contents/Info.plist").open("rb") as f:
        info = plistlib.load(f)
    require(info.get("CFBundleIdentifier") == "org.zipworkspace.desktop"
            and info.get("CFBundleExecutable") == "ZipWorkspace" and info.get("CFBundleVersion") == "1"
            and info.get("CFBundleShortVersionString") == marketing and info.get("LSMinimumSystemVersion") == "26.0",
            "Unexpected macOS bundle identity, version or entry")
    require(info.get("CFBundleDocumentTypes") == [{
                "CFBundleTypeName": "ZIP Workspace Snapshot", "CFBundleTypeRole": "Viewer",
                "LSHandlerRank": "Alternate", "LSItemContentTypes": ["org.zipworkspace.snapshot"],
            }] and info.get("UTExportedTypeDeclarations") == [{
                "UTTypeIdentifier": "org.zipworkspace.snapshot", "UTTypeDescription": "ZIP Workspace Snapshot",
                "UTTypeConformsTo": ["public.data", "public.content", "public.archive"],
                "UTTypeTagSpecification": {"public.filename-extension": ["zxs"]},
            }] and "CFBundleURLTypes" not in info, "Expected only the read-only .zxs document handler")
    exe = app / "Contents/MacOS/ZipWorkspace"
    arch = subprocess.check_output(["/usr/bin/lipo", "-archs", str(exe)], env=env, text=True).strip()
    require(arch == "arm64", "Main executable is not a single arm64 slice")
    subprocess.run(["/usr/bin/codesign", "--verify", "--all-architectures", "--deep", "--strict", str(app)],
                   env=env, check=True)
    display = subprocess.check_output(["/usr/bin/codesign", "--display", "--verbose=4", str(app)],
                                      env=env, stderr=subprocess.STDOUT, text=True).splitlines()
    require("Signature=adhoc" in display and "TeamIdentifier=not set" in display
            and not any(line.startswith("Authority=") for line in display),
            "Expected only a verified local ad-hoc signature, not a distribution identity")


def require_tools(env):
    # Query the existing selection, never invoke --install or change the selected toolchain.
    chosen = Path(subprocess.check_output(["/usr/bin/xcode-select", "-p"], env=env, text=True).strip())
    require(chosen.is_absolute() and chosen.is_dir(), "Select an already installed Apple toolchain separately")
    candidates = (chosen / "usr/bin", chosen / "Toolchains/XcodeDefault.xctoolchain/usr/bin")
    require(any(all((d / tool).is_file() and os.access(d / tool, os.X_OK)
                    for tool in ("lipo", "install_name_tool")) for d in candidates),
            "Preinstall the native linker tools; no install fallback")


def build(out, *, allow_local_adhoc):
    require(allow_local_adhoc, "This local profile needs explicit --allow-local-adhoc; no credentials or Developer ID")
    release, marketing, wheels = profile()
    # The operator provides a complete, offline, preinstalled toolchain and an exclusive output parent.
    require(out.is_absolute() and out.parent != out and out.parent.resolve(strict=True) == out.parent
            and out.resolve() == out and not out.is_relative_to(PROJECT), "Use a new absolute path under a stable private parent")
    env = {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": str(Path.home()), "LC_ALL": "C"}
    require_tools(env)
    out.mkdir(mode=0o700)
    tmp, empty_upx = out / "temp", out / "empty-upx"
    tmp.mkdir(mode=0o700)
    empty_upx.mkdir(mode=0o700)
    env.update(TMPDIR=str(tmp), PYINSTALLER_CONFIG_DIR=str(out / "tool-cache"),
               PYINSTALLER_STRICT_BUNDLE_CODESIGN_ERROR="1", ZIPWORKSPACE_ALLOW_LOCAL_ADHOC="1")
    dist = out / "dist"
    subprocess.run([sys.executable, "-I", "-B", "-m", "PyInstaller",
                    "--upx-dir", str(empty_upx), "--workpath", str(out / "work"),
                    "--distpath", str(dist), str(HERE / "ZipWorkspace.spec")], cwd=PROJECT, env=env, check=True)
    app = dist / "ZipWorkspace.app"
    verify_app(app, marketing, env)
    archive = out / ("zip-workspace-" + release + "-macos-arm64-candidate.zip")
    # ditto preserves the native app layout and its internal links; never zip the intermediate COLLECT tree.
    subprocess.run(["/usr/bin/ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", str(app), str(archive)],
                   env=env, check=True)
    meta = archive.lstat()
    require(stat.S_ISREG(meta.st_mode) and meta.st_size > 0, "Missing regular candidate ZIP")
    with archive.open("rb") as f:
        digest = hashlib.file_digest(f, "sha256").hexdigest()
    record = {"file": archive.name, "version": release, "platform": "macos-arm64", "bundle": app.name,
              "signature": "ad-hoc (local only)", "bytes": meta.st_size, "sha256": digest, "build_wheels": wheels}
    with (out / "candidate.json").open("x", encoding="utf-8") as f:
        json.dump(record, f, indent=2)
        f.write("\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True, help="new directory below a stable private output parent")
    parser.add_argument("--allow-local-adhoc", action="store_true", help="allow the local, credential-free ad-hoc signature")
    try:
        args = parser.parse_args()
        build(args.output, allow_local_adhoc=args.allow_local_adhoc)
    except (OSError, ValueError, PackageNotFoundError, subprocess.SubprocessError) as err:
        parser.exit(1, f"No complete candidate: {err}\n")
