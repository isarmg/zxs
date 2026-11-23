"""Offline native candidate. Link one application PYD; never load it, start a service, sign or ship a driver."""
import argparse
import hashlib
from pathlib import Path
import os
import subprocess
import sys
import sysconfig
sys.path.insert(0, str(Path(__file__).resolve().parent))  # trusted sibling recipe, including under -I
from build import PROJECT, native_support, ordinary, profile, require
import stat

# WinFsp SDK commit ddca7bd5481857a65ba552f643b8776fd070836f (v2.1), exact upstream public headers.
HEADERS = {"winfsp.h": "163332ebe058dd6e49d038edefef273454912c2ec44e8430a18ed73112e5678c",
           "fsctl.h": "7a83a69cdeb155751032eedbbb6875234b1e0824be7cf2d7ba23c1ef44944651"}


def digest(path):
    ordinary(path, stat.S_IFREG)
    with path.open("rb") as src:
        return hashlib.file_digest(src, "sha256").hexdigest()


def build(out, sdk, import_hash):
    profile()
    require(out.is_absolute() and not out.exists() and out.parent.resolve(strict=True) == out.parent
            and sdk.is_absolute() and sdk.resolve(strict=True) == sdk, "select an original prepared SDK and fresh output")
    require(os.environ.get("VSCMD_ARG_HOST_ARCH") == os.environ.get("VSCMD_ARG_TGT_ARCH") == "x64",
            "run in the independently vetted native x64 Visual Studio tools environment")
    include, lib = sdk / "inc", sdk / "lib" / "winfsp-x64.lib"
    for leaf, expected in HEADERS.items():
        require(digest(include / "winfsp" / leaf) == expected, "unexpected WinFsp v2.1 public header")
    require(len(import_hash) == 64 and digest(lib) == import_hash, "vendor import library differs from independent SDK receipt")
    env = {k: v for k, v in os.environ.items() if k.upper() not in ("CL", "_CL_", "LINK", "_LINK_")}
    prefix = Path(os.environ["VCToolsInstallDir"])
    compiler = prefix / "bin" / "Hostx64" / "x64" / "cl.exe"
    ordinary(compiler, stat.S_IFREG)
    out.mkdir()
    source = PROJECT / "winfsp"
    product = out / native_support().PROVIDER
    python = Path(sys.base_prefix)
    subprocess.run([str(compiler), "/nologo", "/TC", "/std:c17", "/W3", "/O2", "/MD",
                    "/guard:cf", "/GS", "/LD", "/I" + str(include), "/I" + sysconfig.get_path("include"),
                    str(source / "draft_host.c"), str(source / "draft_io.c"), "/link", "/MACHINE:X64",
                    "/INCREMENTAL:NO", "/DYNAMICBASE", "/NXCOMPAT", "/guard:cf", "/OUT:" + str(product),
                    str(lib), str(python / "libs/python314.lib"), "advapi32.lib"], cwd=out, env=env, check=True)
    ordinary(product, stat.S_IFREG)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--sdk", type=Path, required=True)
    parser.add_argument("--import-lib-sha256", required=True)
    args = parser.parse_args()
    build(args.output, args.sdk, args.import_lib_sha256)
