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
from xml.etree import ElementTree

HERE = Path(__file__).resolve().parent
PROJECT = HERE.parents[1]
PIN = re.compile(r"([A-Za-z0-9_-]+)==([0-9.]+) --hash=sha256:[0-9a-f]{64}")
AMD64 = 0x8664
MAX_MANIFEST = 64 * 1024


def require(ok, reason):
    if not ok:
        raise ValueError(reason)


def native_amd64():
    # The interpreter platform string describes the process, not the host.
    from ctypes import POINTER, WinDLL, WinError, byref, get_last_error
    from ctypes.wintypes import BOOL, HANDLE, USHORT
    kernel = WinDLL("kernel32.dll", use_last_error=True, winmode=0x800)  # SYSTEM32
    current = kernel.GetCurrentProcess
    current.argtypes, current.restype = (), HANDLE
    try:
        machine = kernel.IsWow64Process2
    except AttributeError as err:
        raise ValueError("Cannot verify native Windows x64") from err
    machine.argtypes, machine.restype = (HANDLE, POINTER(USHORT), POINTER(USHORT)), BOOL
    process, host = USHORT(), USHORT()
    if not machine(current(), byref(process), byref(host)):
        raise WinError(get_last_error())
    return process.value == 0 and host.value == AMD64


def profile():
    require(sys.platform == "win32" and sys.getwindowsversion().build >= 22000
            and sys.getwindowsversion().product_type == 1
            and sys.implementation.name == "cpython" and sys.version_info[:3] == (3, 14, 10)
            and sys.version_info.releaselevel == "final" and sysconfig.get_platform() == "win-amd64"
            and not sysconfig.get_config_var("Py_GIL_DISABLED"), "Requires Windows 11 x64, official CPython 3.14.10 x64 with GIL")
    require(native_amd64(), "Requires a native x64 Windows host, not interpreter emulation")
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
    require(len(pins) == 15, "Incomplete Windows wheel lock")
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


def require_invoker_manifest(raw):
    require(0 < len(raw) <= MAX_MANIFEST, "Missing or oversized executable manifest")
    text = raw.decode("utf-8-sig")
    require("<!DOCTYPE" not in text and "\0" not in text, "Unexpected executable manifest encoding or DTD")
    try:
        root = ElementTree.fromstring(text)
    except ElementTree.ParseError as err:
        raise ValueError("Malformed executable manifest") from err
    require(root.tag == "{urn:schemas-microsoft-com:asm.v1}assembly"
            and root.get("manifestVersion") == "1.0", "Invalid executable manifest root")
    ns = "{urn:schemas-microsoft-com:asm.v3}"
    node = root
    for local in ("trustInfo", "security", "requestedPrivileges", "requestedExecutionLevel"):
        matches = [child for child in node if child.tag.rsplit("}", 1)[-1] == local]
        require(len(matches) == 1 and matches[0].tag == ns + local,
                "Missing or ambiguous executable manifest trust policy")
        node = matches[0]
    levels = [entry for entry in root.iter() if entry.tag.rsplit("}", 1)[-1] == "requestedExecutionLevel"]
    require(levels == [node] and node.attrib == {"level": "asInvoker", "uiAccess": "false"}
            and not len(node) and not (node.text or "").strip(), "Executable must launch asInvoker without UIAccess")


def require_entry_image(path):
    from pefile import DIRECTORY_ENTRY, PE, PEFormatError

    try:
        image = PE(str(path), fast_load=True)
        try:
            require(image.FILE_HEADER.Machine == AMD64 and image.OPTIONAL_HEADER.Magic == 0x20B
                    and image.OPTIONAL_HEADER.Subsystem == 2,
                    "Result is not an x64 Windows GUI executable")
            resource = DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_RESOURCE"]
            require(resource < len(image.OPTIONAL_HEADER.DATA_DIRECTORY), "Missing executable resource directory")
            directory = image.OPTIONAL_HEADER.DATA_DIRECTORY[resource]
            start, length = directory.VirtualAddress, directory.Size
            section = image.get_section_by_rva(start)
            require(section is not None and length > 0, "Missing executable resource section")
            delta = start - section.VirtualAddress
            extent = min(section.Misc_VirtualSize, section.SizeOfRawData)
            disk = section.PointerToRawData + delta
            require(0 <= delta and delta + length <= extent and 0 <= disk
                    and disk + length <= path.stat().st_size and section.get_offset_from_rva(start) == disk,
                    "Resource directory leaves the section's declared raw and virtual extent")

            def in_directory(record):
                at = record.get_file_offset()
                require(disk <= at and at + record.sizeof() <= disk + length,
                        "Resource structure outside the executable resource directory")

            def entries(owner, attr):
                tree = getattr(owner, attr, None)
                require(tree is not None and len(tree.entries) ==
                        tree.struct.NumberOfNamedEntries + tree.struct.NumberOfIdEntries,
                        "Missing or incomplete executable resource table")
                in_directory(tree.struct)
                for item in tree.entries:
                    in_directory(item.struct)
                return tree.entries

            image.parse_data_directories(directories=[resource])
            manifests = [item for item in entries(image, "DIRECTORY_ENTRY_RESOURCE") if item.id == 24]
            require(len(manifests) == 1, "Expected one embedded RT_MANIFEST")
            names = entries(manifests[0], "directory")
            require(len(names) == 1 and names[0].id == 1, "Expected the executable's CreateProcess manifest")
            languages = entries(names[0], "directory")
            ids = {item.id for item in languages}
            require(0 in ids and len(ids) == len(languages)
                    and all(isinstance(i, int) and 0 <= i <= 65535 for i in ids),
                    "Ambiguous executable manifest languages")
            for item in languages:
                data = getattr(item, "data", None)
                require(data is not None and 0 < data.struct.Size <= MAX_MANIFEST, "Missing executable manifest bytes")
                in_directory(data.struct)
                rva, size = data.struct.OffsetToData, data.struct.Size
                offset = rva - section.VirtualAddress
                require(0 <= offset and offset + size <= extent,
                        "Executable manifest leaves the section's declared raw and virtual extent")
                require(start <= rva and rva + size <= start + length
                        and section.get_offset_from_rva(rva) == section.PointerToRawData + offset,
                        "Executable manifest outside the executable resource directory")
                raw = section.get_data(rva, size, ignore_padding=True)
                require(len(raw) == size, "Truncated executable manifest")
                require_invoker_manifest(raw)
        finally:
            image.close()
    except PEFormatError as err:
        raise ValueError("Malformed Windows executable image") from err


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
                "_internal/PySide6/plugins/platforms/qwindows.dll", "_internal/COPYRIGHT-NOTICE.txt", "_internal/certifi/cacert.pem"}
    require(required <= files.keys() and all(files[p]["bytes"] > 0 for p in required),
            "Missing executable, Python, Windows Qt platform plugin or notice")
    names = {Path(k).name.casefold() for k in files}
    require({"qt6core.dll", "qt6gui.dll", "qt6widgets.dll"} <= names
            and not any("winfsp" in n or n.endswith(".sys") for n in names), "Wrong Qt runtime or unexpected driver artifact")
    require(not any(k.casefold() == "zipworkspace.exe.manifest" for k in files), "Unexpected external launch manifest")
    require_entry_image(bundle / "ZipWorkspace.exe")
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
