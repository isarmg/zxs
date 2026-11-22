#!/usr/bin/env python3
"""Stage the four ZXS browser resources without building the native executable."""

import argparse
import ctypes
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import stat
import subprocess
import sys
import tarfile
import tempfile
import tomllib

sys.dont_write_bytecode = True
from build_release import (BROWSER_ASSETS, BROWSER_AUTHORED as AUTHORED, BROWSER_RECEIPT,
                           BROWSER_TARGET as TARGET, MAX_BROWSER_FILE, RUST, RUST_COMMIT,
                           TARGET as HOST, browser_inputs, browser_recipe_inputs, directory,
                           need, require_name, sha256, text)

STD_SHA = "19b7954d650fc0d22767eceb76b8e03d07ce0d6a9f4234f33c32a17ee8d6d7b9"
BINDGEN = "0.2.129"
BINDGEN_ARCHIVE_SHA = "82d12bb940e2d4e72e0d5605387fc1b8ca179044e012b620f0ce4e7440e8320e"
BINDGEN_SHA = "fb59da714982a04273e9e4b09a85e523e5dd04de35b1a4a8b1641785ee7a71cf"
GENERATED = BROWSER_ASSETS[len(AUTHORED):]
OUTPUT = "browser-assets"
MIB = 1024 * 1024
REGISTRY = "registry+https://github.com/rust-lang/crates.io-index"


def verified_tar(path, checksum, limit, mode):
    # Parse the same bytes that were compared to the reviewed digest.
    info = path.lstat()
    need(stat.S_ISREG(info.st_mode) and 0 < info.st_size <= limit, f"invalid archive: {path}")
    with path.open("rb") as file:
        raw = file.read(limit + 1)
    need(len(raw) == info.st_size and hashlib.sha256(raw).hexdigest() == checksum,
         f"archive digest mismatch: {path}")
    return tarfile.open(fileobj=io.BytesIO(raw), mode=mode)


def unpack(archive, prefix, dest):
    """Keep regular files under this exact package prefix, never links or installer actions."""
    prefix = PurePosixPath(prefix)
    files = {}
    dest.mkdir()
    for member in archive:
        parts = PurePosixPath(member.name)
        if parts == prefix or not parts.is_relative_to(prefix):
            continue
        name = parts.relative_to(prefix)
        need(".." not in name.parts and (member.isdir() or member.isfile()),
             f"unexpected archive member: {member.name}")
        if member.isdir():
            continue
        leaf = dest.joinpath(*name.parts)
        leaf.parent.mkdir(parents=True, exist_ok=True)
        with archive.extractfile(member) as inp, leaf.open("xb") as out:
            body = inp.read()
            need(len(body) == member.size, f"short archive member: {member.name}")
            out.write(body)
        files[str(name)] = hashlib.sha256(body).hexdigest()
    need(files, f"empty archive portion: {prefix}")
    return files


def supply_crates(lock, cache, vendor):
    packages = tomllib.loads(lock.read_text(encoding="utf-8"))
    need(packages.get("version") == 4, "expected Cargo.lock format 4")
    local = []
    crates = {}
    vendor.mkdir()
    for package in packages["package"]:
        name, version = package["name"], package["version"]
        if "source" not in package:
            local.append(name)
            continue
        checksum = package.get("checksum", "")
        need(package["source"] == REGISTRY and re.fullmatch(r"[A-Za-z0-9_-]+", name) and
             re.fullmatch(r"[0-9][A-Za-z0-9.+-]*", version) and re.fullmatch(r"[0-9a-f]{64}", checksum),
             f"unsupported browser lock entry: {name}")
        stem = f"{name}-{version}"
        with verified_tar(cache / f"{stem}.crate", checksum, 8 * MIB, "r:gz") as archive:
            files = unpack(archive, stem, vendor / stem)
        with (vendor / stem / ".cargo-checksum.json").open("x", encoding="utf-8") as out:
            json.dump({"package": checksum, "files": files}, out)
        crates[stem] = checksum
    need(local == ["zxs-browser"] and f"wasm-bindgen-{BINDGEN}" in crates,
         "unexpected browser workspace or wasm-bindgen lock")
    return crates


def reject_cargo_config(cwd):
    for parent in (cwd, *cwd.parents):
        for name in ("config", "config.toml"):
            config = parent / ".cargo" / name
            need(not os.path.lexists(config), f"ambient Cargo configuration is not a build input: {config}")


def publish(stage, output):
    # Publish all four resources and their receipt as one no-replace directory.
    rename = getattr(ctypes.CDLL(None, use_errno=True), "renameat2", None)
    need(rename is not None, "no renameat2; cannot publish browser assets without replacement")
    rename.argtypes = (ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint)
    rename.restype = ctypes.c_int
    if rename(stage, b"ready", output, os.fsencode(OUTPUT), 1) != 0:  # RENAME_NOREPLACE
        err = ctypes.get_errno()
        raise OSError(err, os.strerror(err), OUTPUT)


def compile_browser(source, stage, bindir, std, bindgen_input, cache):
    original = source / "browser"
    browser = stage / "browser"
    need(not os.path.lexists(original / "build.rs"), "browser build scripts are not supported")
    inputs = browser_inputs(original, browser)
    manifest = tomllib.loads((browser / "Cargo.toml").read_text(encoding="utf-8"))
    need(manifest["package"].get("build", False) is False and
         manifest["lib"].get("path", "src/lib.rs") == "src/lib.rs", "unexpected browser build entry")
    reject_cargo_config(browser)
    cargo, rustc, rustdoc = (bindir / name for name in ("cargo", "rustc", "rustdoc"))
    toolchain = bindir.parent
    host_lib = toolchain / "lib" / "rustlib" / HOST
    lld = host_lib / "bin" / "rust-lld"
    need(all(p.is_file() and os.access(p, os.X_OK) for p in (cargo, rustc, rustdoc, lld)),
         "pass the reviewed local Rust 1.100.0 bin directory")
    for name in ("home", "cargo", "tmp", "generated", "ready"):
        (stage / name).mkdir(mode=0o700)
    sysroot = stage / "sysroot"
    rustlibs = sysroot / "lib" / "rustlib"
    rustlibs.mkdir(parents=True)
    (rustlibs / HOST).symlink_to(host_lib, target_is_directory=True)
    with verified_tar(std, STD_SHA, 32 * MIB, "r:xz") as archive:
        unpack(archive, f"rust-std-{RUST}-{TARGET}/rust-std-{TARGET}/lib/rustlib/{TARGET}",
               rustlibs / TARGET)

    bindgen = stage / "wasm-bindgen"
    with verified_tar(bindgen_input, BINDGEN_ARCHIVE_SHA, 16 * MIB, "r:gz") as archive:
        member = archive.getmember(f"wasm-bindgen-{BINDGEN}-x86_64-unknown-linux-musl/wasm-bindgen")
        need(member.isfile(), "invalid wasm-bindgen executable member")
        with archive.extractfile(member) as inp:
            bindgen.write_bytes(inp.read())
    need(sha256(bindgen) == BINDGEN_SHA, "wasm-bindgen executable digest mismatch")
    bindgen.chmod(0o700)

    vendor = stage / "vendor"
    crates = supply_crates(browser / "Cargo.lock", cache, vendor)
    (stage / "cargo" / "config.toml").write_text(
        '[source.crates-io]\nreplace-with = "verified-browser-input"\n'
        '[source.verified-browser-input]\ndirectory = ' + json.dumps(str(vendor), ensure_ascii=False) + "\n",
        encoding="utf-8")
    env = {
        "PATH": "/usr/bin:/bin", "HOME": str(stage / "home"), "CARGO_HOME": str(stage / "cargo"),
        "TMPDIR": str(stage / "tmp"), "LC_ALL": "C", "TZ": "UTC", "SOURCE_DATE_EPOCH": "0",
        "RUSTC": str(rustc), "RUSTDOC": str(rustdoc), "CARGO_TARGET_DIR": str(stage / "target"),
        "CARGO_BUILD_JOBS": "2", "CARGO_INCREMENTAL": "0",
        "CARGO_ENCODED_RUSTFLAGS": "\x1f".join((
            f"--sysroot={sysroot}", "-C", f"linker={lld}",
            f"--remap-path-prefix={browser}=/zxs/browser",
            f"--remap-path-prefix={stage}=/zxs/browser-build",
            f"--remap-path-prefix={toolchain}=/zxs/rust",
            f"--remap-path-prefix={vendor}=/zxs-browser-deps",
        )),
    }
    rust = text([rustc, "-vV"], env).strip()
    for line in (f"release: {RUST}", f"commit-hash: {RUST_COMMIT}", f"host: {HOST}"):
        need(line in rust.splitlines(), "unsupported Rust toolchain")
    need(Path(text([rustc, "--print", "sysroot"], env).strip()).resolve() == toolchain,
         "use the direct reviewed Rust bin directory, not a toolchain proxy")
    cargo_version = text([cargo, "-V"], env).strip()
    need(cargo_version.startswith(f"cargo {RUST} "), "unsupported Cargo version")
    bindgen_version = text([bindgen, "--version"], env).strip()
    need(bindgen_version == f"wasm-bindgen {BINDGEN}", "unsupported wasm-bindgen CLI")

    subprocess.run([cargo, "build", "--locked", "--offline", "--release", "--target", TARGET, "--lib"],
                   cwd=browser, env=env, check=True)
    generated = stage / "generated"
    wasm = stage / "target" / TARGET / "release" / "zxs_browser.wasm"
    subprocess.run([bindgen, "--target", "web", "--no-typescript", "--out-name", "zxs_browser",
                    "--out-dir", generated, wasm], cwd=stage, env=env, check=True)
    need({p.name for p in generated.iterdir()} == set(GENERATED), "unexpected wasm-bindgen outputs")

    files = {}
    for name in BROWSER_ASSETS:
        file = (browser if name in AUTHORED else generated) / name
        info = file.lstat()
        need(stat.S_ISREG(info.st_mode) and 0 < info.st_size <= MAX_BROWSER_FILE, f"invalid browser asset: {name}")
        data = file.read_bytes()
        if name.endswith(".wasm"):
            need(data.startswith(b"\0asm\x01\0\0\0"), "invalid WebAssembly module")
        else:
            data.decode("utf-8")
        checksum = hashlib.sha256(data).hexdigest()
        if name in AUTHORED:
            need(checksum == inputs[name], f"browser source changed: {name}")
        (stage / "ready" / name).write_bytes(data)
        files[name] = {"bytes": len(data), "sha256": checksum}
    need(browser_inputs(browser) == inputs and browser_inputs(original) == inputs,
         "browser source changed during build")
    receipt = {
        "format": 1, "target": TARGET, "source": inputs, "registry_archives": crates,
        "recipe": browser_recipe_inputs(source),
        "tools": {
            "rustc": {"version": rust, "sha256": sha256(rustc)},
            "cargo": {"version": cargo_version, "sha256": sha256(cargo)},
            "rust_lld_sha256": sha256(lld),
            "rust_std_archive_sha256": STD_SHA,
            "wasm_bindgen": {"version": bindgen_version, "archive_sha256": BINDGEN_ARCHIVE_SHA,
                             "sha256": BINDGEN_SHA},
        },
        "assets": files,
    }
    (stage / "ready" / BROWSER_RECEIPT).write_text(
        json.dumps(receipt, sort_keys=True, indent=2) + "\n", encoding="utf-8")


def build(source, bindir, std, bindgen, cache):
    host = os.uname()
    need((host.sysname, host.machine) == ("Linux", "x86_64"), "use a Linux x86-64 build host")
    need(os.getuid() == os.geteuid() != 0, "build in a private, unprivileged worktree")
    with directory(source) as root, directory("browser", root) as crate, \
         directory("target", root, create=True) as target:
        try:
            os.stat(OUTPUT, dir_fd=target, follow_symlinks=False)
        except FileNotFoundError:
            pass
        else:
            raise ValueError(f"refusing to replace {source / 'target' / OUTPUT}")
        with tempfile.TemporaryDirectory(prefix=".zxs-browser-", dir=source / "target") as tmp:
            stage = Path(tmp)
            with directory(stage.name, target) as held:
                compile_browser(source, stage, bindir, std, bindgen, cache)
                require_name(source, root)
                require_name("browser", crate, root)
                require_name("target", target, root)
                require_name(stage.name, held, target)
                publish(held, target)
    print(source / "target" / OUTPUT)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("toolchain_bin", type=Path, help="reviewed local Rust 1.100.0 bin directory")
    ap.add_argument("--wasm-std", type=Path, required=True, help="official wasm32 Rust standard library archive")
    ap.add_argument("--wasm-bindgen", type=Path, required=True, help="official wasm-bindgen Linux musl archive")
    ap.add_argument("--crates", type=Path, required=True, help="directory of the official .crate archives in Cargo.lock")
    args = ap.parse_args()
    try:
        build(Path(__file__).resolve().parent, args.toolchain_bin.resolve(strict=True),
              args.wasm_std, args.wasm_bindgen, args.crates)
    except (OSError, ValueError, KeyError, tarfile.TarError, subprocess.CalledProcessError) as error:
        ap.exit(1, f"ZXS browser build refused: {error}\n")


if __name__ == "__main__":
    main()
