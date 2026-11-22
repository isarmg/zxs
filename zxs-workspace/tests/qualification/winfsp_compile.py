"""Compile the WinFsp C sources to x64 COFF objects on the Windows hosted lane."""
import hashlib
import os
from pathlib import Path
import subprocess
import sys
import sysconfig
import tempfile
import unittest
from urllib.request import urlopen

HERE = Path(__file__).resolve()
PROJECT = HERE.parents[2]
SDK = "https://raw.githubusercontent.com/winfsp/winfsp/ddca7bd5481857a65ba552f643b8776fd070836f/inc/winfsp/"
HEADERS = {
    "winfsp.h": "163332ebe058dd6e49d038edefef273454912c2ec44e8430a18ed73112e5678c",
    "fsctl.h": "7a83a69cdeb155751032eedbbb6875234b1e0824be7cf2d7ba23c1ef44944651",
}
UNITS = ("winfsp/provider.c", "winfsp/lifecycle.c", "tests/qualification/winfsp_callbacks.c")


def require(ok, message):
    if not ok:
        raise RuntimeError(message)


def sdk_headers(root):
    directory = root / "winfsp"
    directory.mkdir()
    for name, wanted in HEADERS.items():
        url = SDK + name
        with urlopen(url, timeout=20) as reply:
            require(reply.geturl() == url, "unexpected WinFsp header origin")
            content = reply.read(1024 * 1024 + 1)
        require(0 < len(content) <= 1024 * 1024 and hashlib.sha256(content).hexdigest() == wanted,
                "vendor header differs from WinFsp v2.1: " + name)
        (directory / name).write_bytes(content)


def compiler_environment():
    seed = {k.upper(): v for k, v in os.environ.items()
            if k.upper() not in ("CL", "_CL_", "LINK", "_LINK_", "INCLUDE", "LIB", "LIBPATH")}
    cmd = Path(seed["SYSTEMROOT"]) / "System32/cmd.exe"
    vswhere = Path(seed["PROGRAMFILES(X86)"]) / "Microsoft Visual Studio/Installer/vswhere.exe"
    output = subprocess.check_output([
        vswhere, "-latest", "-products", "*", "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
        "-property", "installationPath", "-utf8",
    ], env=seed, timeout=20).decode("utf-8-sig")
    rows = output.splitlines()
    require(len(rows) == 1, "expected the hosted image's Visual Studio x64 toolset")
    installation = Path(rows[0]).resolve(strict=True)
    developer = installation / "Common7/Tools/VsDevCmd.bat"
    require(developer.is_file() and not any(c in str(developer) for c in '%!"\r\n'),
            "unexpected Visual Studio command path")
    command = f'"{cmd}" /d /u /s /c ""{developer}" -no_logo -arch=x64 -host_arch=x64 >nul && set"'
    output = subprocess.check_output(command, executable=cmd, env=seed, timeout=60).decode("utf-16-le")
    env = {}
    for line in output.splitlines():
        name, equal, value = line.partition("=")
        if name and equal:
            env[name.upper()] = value
    require(env.get("VSCMD_ARG_HOST_ARCH") == env.get("VSCMD_ARG_TGT_ARCH") == "x64",
            "expected x64 host and target")
    for key in ("CL", "_CL_", "LINK", "_LINK_"):
        env.pop(key, None)
    compiler = (Path(env["VCTOOLSINSTALLDIR"]) / "bin/Hostx64/x64/cl.exe").resolve(strict=True)
    require(compiler.is_relative_to(installation / "VC/Tools/MSVC"), "compiler is not from the selected VS installation")
    return compiler, env


@unittest.skipUnless(os.environ.get("GITHUB_ACTIONS") == "true" and sys.platform == "win32"
                     and sys.getwindowsversion().product_type in (2, 3)
                     and sysconfig.get_platform() == "win-amd64", "requires the GitHub-hosted x64 Server lane")
class NativeSdkCompiler(unittest.TestCase):
    def test_actual_sdk_and_cpython_headers_compile_three_x64_objects(self):
        compiler, env = compiler_environment()
        python = Path(sysconfig.get_path("include")).resolve(strict=True)
        require((python / "Python.h").is_file() and (python / "pyconfig.h").is_file(),
                "missing hosted CPython headers")
        with tempfile.TemporaryDirectory(prefix="workspace-winfsp-source-") as work:
            root = Path(work).resolve(strict=True)
            sdk_headers(root)
            for unit in UNITS:
                out = root / (Path(unit).stem + ".obj")
                with self.subTest(unit=unit):
                    subprocess.run([compiler, "/nologo", "/TC", "/std:c17", "/W3", "/c",
                                    "/I" + str(root), "/I" + str(python),
                                    "/Fo" + str(out), PROJECT / unit],
                                   cwd=root, env=env, check=True, timeout=90)
                    with out.open("rb") as obj:
                        self.assertEqual(obj.read(2), b"\x64\x86", "expected an AMD64 COFF object")
            self.assertEqual(sorted(p.name for p in root.iterdir()),
                             sorted(["winfsp", *(Path(unit).stem + ".obj" for unit in UNITS)]))
        print("WinFsp v2.1 headers and CPython: three AMD64 objects compiled; no linker, DLL or driver.", flush=True)
