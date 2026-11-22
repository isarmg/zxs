"""Run strict Clippy on a private standalone browser snapshot; never stage assets."""

import os
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib

SOURCE = Path(__file__).resolve().parents[2]
sys.dont_write_bytecode = True
sys.path.insert(0, str(SOURCE))
from build_browser import (HOST, MIB, RUST, RUST_COMMIT, STD_SHA, TARGET, browser_inputs,
                           need, reject_cargo_config, text, unpack, verified_tar)


def lint(bindir, std):
    need((os.uname().sysname, os.uname().machine) == ("Linux", "x86_64")
         and os.getuid() == os.geteuid() != 0, "use an unprivileged Linux x86-64 runner")
    original = SOURCE / "browser"
    with tempfile.TemporaryDirectory(prefix="zxs-browser-clippy-") as tmp:
        scratch = Path(tmp).resolve(strict=True)
        browser = scratch / "browser"
        reject_cargo_config(browser)
        inputs = browser_inputs(original, browser)
        manifest = tomllib.loads((browser / "Cargo.toml").read_text("utf-8"))
        lock = tomllib.loads((browser / "Cargo.lock").read_text("utf-8"))
        need(not os.path.lexists(original / "build.rs")
             and manifest["package"].get("build", False) is False
             and manifest["lib"].get("path", "src/lib.rs") == "src/lib.rs"
             and lock == {"version": 4, "package": [
                 {"name": "zxs-browser", "version": manifest["package"]["version"]}
             ]}, "unexpected browser build entry or dependency")
        for name in ("cargo", "home", "tmp"):
            (scratch / name).mkdir(mode=0o700)
        sysroot = scratch / "sysroot"
        libs = sysroot / "lib" / "rustlib"
        libs.mkdir(parents=True)
        (libs / HOST).symlink_to(bindir.parent / "lib" / "rustlib" / HOST, target_is_directory=True)
        with verified_tar(std, STD_SHA, 32 * MIB, "r:xz") as archive:
            unpack(archive, f"rust-std-{RUST}-{TARGET}/rust-std-{TARGET}/lib/rustlib/{TARGET}", libs / TARGET)

        rustc = bindir / "rustc"
        env = {
            "PATH": f"{bindir}:/usr/bin:/bin", "LC_ALL": "C", "HOME": str(scratch / "home"),
            "CARGO_HOME": str(scratch / "cargo"), "CARGO_TARGET_DIR": str(scratch / "target"),
            "TMPDIR": str(scratch / "tmp"), "RUSTC": str(rustc), "RUSTDOC": str(bindir / "rustdoc"),
            "CARGO_ENCODED_RUSTFLAGS": f"--sysroot={sysroot}",
        }
        version = text([rustc, "-vV"], env).splitlines()
        need(all(line in version for line in
                 (f"release: {RUST}", f"commit-hash: {RUST_COMMIT}", f"host: {HOST}"))
             and Path(text([rustc, "--print", "sysroot"], env).strip()).resolve() == bindir.parent,
             "use the reviewed direct Rust toolchain")
        subprocess.run([bindir / "cargo", "clippy", "--manifest-path", browser / "Cargo.toml",
                        "--target", TARGET, "--lib", "--locked", "--offline", "--",
                        "-D", "warnings", "-D", "clippy::pedantic"], cwd=browser, env=env, check=True)
        need(browser_inputs(original) == inputs and browser_inputs(browser) == inputs,
             "browser source changed during Clippy")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit("usage: strict_target.py REVIEWED_TOOLCHAIN_BIN VERIFIED_WASM_STD_ARCHIVE")
    lint(Path(sys.argv[1]).resolve(strict=True), Path(sys.argv[2]))
