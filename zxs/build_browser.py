#!/usr/bin/env python3
"""Stage the four ZXS browser resources without building the native executable."""

import argparse
from contextlib import contextmanager
import ctypes
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import stat
import subprocess
import sys
import tarfile
import tempfile

sys.dont_write_bytecode = True
from build_release import (BROWSER_ASSETS, BROWSER_RECEIPT, BROWSER_SOURCE,
                           BROWSER_TARGET as TARGET, MAX_BROWSER_FILE, RUST, RUST_COMMIT,
                           TARGET as HOST, browser_inputs, browser_recipe_inputs, directory,
                           need, require_name, sha256, text)

STD_SHA = "19b7954d650fc0d22767eceb76b8e03d07ce0d6a9f4234f33c32a17ee8d6d7b9"
AUTHORED = {"boot.js": "boot.js", "folder.css": "folder.css", "zxs_browser.js": "console_host.js"}
OUTPUT = "browser-assets"
MIB = 1024 * 1024


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


@contextmanager
def checked_browser_source(source):
    with directory(source) as root, directory("src", root) as src, directory("browser", src) as frontend:
        reject_cargo_config(source)
        inputs = browser_inputs(source)
        yield inputs
        require_name(source, root)
        require_name("src", src, root)
        require_name("browser", frontend, src)
        reject_cargo_config(source)
        need(browser_inputs(source) == inputs, "browser build inputs changed")


def browser_environment(source, stage, bindir, std):
    host = os.uname()
    need((host.sysname, host.machine) == ("Linux", "x86_64"), "use a Linux x86-64 build host")
    need(os.getuid() == os.geteuid() != 0, "build in a private, unprivileged worktree")
    cargo, rustc, rustdoc = (bindir / name for name in ("cargo", "rustc", "rustdoc"))
    toolchain = bindir.parent
    host_lib = toolchain / "lib" / "rustlib" / HOST
    lld = host_lib / "bin" / "rust-lld"
    need(all(p.is_file() and os.access(p, os.X_OK) for p in (cargo, rustc, rustdoc, lld)),
         "pass the reviewed local Rust 1.100.0 bin directory")
    for name in ("home", "cargo", "tmp"):
        (stage / name).mkdir(mode=0o700)
    sysroot = stage / "sysroot"
    rustlibs = sysroot / "lib" / "rustlib"
    rustlibs.mkdir(parents=True)
    (rustlibs / HOST).symlink_to(host_lib, target_is_directory=True)
    with verified_tar(std, STD_SHA, 32 * MIB, "r:xz") as archive:
        unpack(archive, f"rust-std-{RUST}-{TARGET}/rust-std-{TARGET}/lib/rustlib/{TARGET}",
               rustlibs / TARGET)

    env = {
        "PATH": f"{bindir}:/usr/bin:/bin", "HOME": str(stage / "home"), "CARGO_HOME": str(stage / "cargo"),
        "TMPDIR": str(stage / "tmp"), "LC_ALL": "C", "TZ": "UTC", "SOURCE_DATE_EPOCH": "0",
        "RUSTC": str(rustc), "RUSTDOC": str(rustdoc), "CARGO_TARGET_DIR": str(stage / "target"),
        "CARGO_BUILD_JOBS": "2", "CARGO_INCREMENTAL": "0",
        "CARGO_ENCODED_RUSTFLAGS": "\x1f".join((
            f"--sysroot={sysroot}", "-C", f"linker={lld}",
            f"--remap-path-prefix={source}=/storage-server/source",
            f"--remap-path-prefix={stage}=/storage-server/browser-build",
            f"--remap-path-prefix={toolchain}=/storage-server/rust",
        )),
    }
    rust = text([rustc, "-vV"], env).strip()
    for line in (f"release: {RUST}", f"commit-hash: {RUST_COMMIT}", f"host: {HOST}"):
        need(line in rust.splitlines(), "unsupported Rust toolchain")
    need(Path(text([rustc, "--print", "sysroot"], env).strip()).resolve() == toolchain,
         "use the direct reviewed Rust bin directory, not a toolchain proxy")
    cargo_version = text([cargo, "-V"], env).strip()
    need(cargo_version.startswith(f"cargo {RUST} "), "unsupported Cargo version")
    return env, rust, cargo_version, lld


def compile_browser(source, stage, bindir, std, inputs):
    env, rust, cargo_version, lld = browser_environment(source, stage, bindir, std)
    cargo, rustc = bindir / "cargo", bindir / "rustc"
    subprocess.run([cargo, "rustc", "--manifest-path", source / "Cargo.toml", "--locked", "--offline",
                    "--target", TARGET, "--profile", "browser", "--lib", "--crate-type", "cdylib"],
                   cwd=source, env=env, check=True)
    wasm = stage / "target" / TARGET / "browser" / "storage_server.wasm"
    ready = stage / "ready"
    ready.mkdir(mode=0o700)
    files = {}
    for name in BROWSER_ASSETS:
        authored = AUTHORED.get(name)
        file = wasm if authored is None else source / BROWSER_SOURCE / authored
        info = file.lstat()
        need(stat.S_ISREG(info.st_mode) and 0 < info.st_size <= MAX_BROWSER_FILE, f"invalid browser asset: {name}")
        data = file.read_bytes()
        if name.endswith(".wasm"):
            need(data.startswith(b"\0asm\x01\0\0\0"), "invalid WebAssembly module")
        else:
            data.decode("utf-8")
        checksum = hashlib.sha256(data).hexdigest()
        if authored is not None:
            need(checksum == inputs[(BROWSER_SOURCE / authored).as_posix()], f"browser source changed: {authored}")
        (ready / name).write_bytes(data)
        files[name] = {"bytes": len(data), "sha256": checksum}
    receipt = {
        "format": 1, "target": TARGET, "source": inputs,
        "recipe": browser_recipe_inputs(source),
        "tools": {
            "rustc": {"version": rust, "sha256": sha256(rustc)},
            "cargo": {"version": cargo_version, "sha256": sha256(cargo)},
            "rust_lld_sha256": sha256(lld),
            "rust_std_archive_sha256": STD_SHA,
        },
        "assets": files,
    }
    (ready / BROWSER_RECEIPT).write_text(
        json.dumps(receipt, sort_keys=True, indent=2) + "\n", encoding="utf-8")


def build(source, bindir, std):
    with directory(source) as root, directory("target", root, create=True) as target:
        try:
            os.stat(OUTPUT, dir_fd=target, follow_symlinks=False)
        except FileNotFoundError:
            pass
        else:
            raise ValueError(f"refusing to replace {source / 'target' / OUTPUT}")
        with tempfile.TemporaryDirectory(prefix=".zxs-browser-", dir=source / "target") as tmp:
            stage = Path(tmp)
            with directory(stage.name, target) as held:
                with checked_browser_source(source) as inputs:
                    compile_browser(source, stage, bindir, std, inputs)
                require_name(source, root)
                require_name("target", target, root)
                require_name(stage.name, held, target)
                publish(held, target)
    print(source / "target" / OUTPUT)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("toolchain_bin", type=Path, help="reviewed local Rust 1.100.0 bin directory")
    ap.add_argument("--wasm-std", type=Path, required=True, help="official wasm32 Rust standard library archive")
    args = ap.parse_args()
    try:
        build(Path(__file__).resolve().parent, args.toolchain_bin.resolve(strict=True),
              args.wasm_std)
    except (OSError, ValueError, KeyError, tarfile.TarError, subprocess.CalledProcessError) as error:
        ap.exit(1, f"ZXS browser build refused: {error}\n")


if __name__ == "__main__":
    main()
