#!/usr/bin/env python3
"""Build the sole Linux x86-64 ZXS ELF from this reviewed standalone source."""
import argparse
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import tempfile

TARGET = "x86_64-unknown-linux-gnu"
RUST = "1.100.0"
RUST_COMMIT = "0ef1bc386059596dada8fb65b50425aab2ea637a"
MAX_IMAGE = 128 * 1024 * 1024
BROWSER_TARGET = "wasm32-unknown-unknown"
BROWSER_AUTHORED = ("boot.js", "folder.css")
BROWSER_ASSETS = (*BROWSER_AUTHORED, "zxs_browser.js", "zxs_browser_bg.wasm")
BROWSER_RECEIPT = "provenance.json"
MAX_BROWSER_FILE = 8 * 1024 * 1024


def need(ok, why):
    if not ok:
        raise ValueError(why)


def mount_id(fd):
    with open(f"/proc/self/fdinfo/{fd}", "rb") as info:
        raw = info.read(4097)
    found = re.findall(rb"^mnt_id:[ \t]+([1-9][0-9]*)$", raw, re.M)
    need(len(raw) <= 4096 and len(found) == 1, "cannot prove local output mount")
    return int(found[0])


@contextmanager
def directory(path, parent=None, create=False):
    flags = os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC
    try:
        fd = os.open(path, flags, dir_fd=parent)
    except FileNotFoundError:
        if not create:
            raise
        try:
            os.mkdir(path, mode=0o700, dir_fd=parent)
        except FileExistsError:
            pass
        fd = os.open(path, flags, dir_fd=parent)
    try:
        m = os.fstat(fd)
        need(stat.S_ISDIR(m.st_mode) and m.st_uid == os.geteuid() and not m.st_mode & 0o022,
             f"not a private, builder-owned directory: {path}")
        if parent is not None:
            need(mount_id(fd) == mount_id(parent), f"mounted or redirected output directory: {path}")
        yield fd
    finally:
        os.close(fd)


def require_name(path, held, parent=None):
    with directory(path, parent) as current:
        need(os.path.samestat(os.fstat(current), os.fstat(held)) and mount_id(current) == mount_id(held),
             f"build directory changed: {path}")


def text(argv, env):
    return subprocess.check_output(argv, env=env, text=True)


def sha256(path):
    with path.open("rb") as data:
        return hashlib.file_digest(data, "sha256").hexdigest()


def browser_inputs(browser, snapshot=None):
    inputs = {}
    if snapshot is not None:
        snapshot.mkdir(mode=0o700)
    src = browser / "src"
    need(stat.S_ISDIR(src.lstat().st_mode), "missing browser source directory")
    for p in (browser / "Cargo.toml", browser / "Cargo.lock",
              *(browser / name for name in BROWSER_AUTHORED), *sorted(src.rglob("*"))):
        kind = p.lstat().st_mode
        if not stat.S_ISDIR(kind):
            need(stat.S_ISREG(kind), f"not a regular browser source: {p}")
            name = p.relative_to(browser)
            if snapshot is None:
                checksum = sha256(p)
            else:
                data = p.read_bytes()
                staged = snapshot / name
                staged.parent.mkdir(parents=True, exist_ok=True)
                with staged.open("xb") as out:
                    out.write(data)
                checksum = hashlib.sha256(data).hexdigest()
            inputs[name.as_posix()] = checksum
    need(all(name in inputs for name in ("Cargo.toml", "Cargo.lock", "src/lib.rs", *BROWSER_AUTHORED)),
         "incomplete browser source")
    return inputs


def browser_recipe_inputs(source):
    return {name: sha256(source / name) for name in ("build_browser.py", "build_release.py")}


def read_browser_file(held, name):
    expected = os.stat(name, dir_fd=held, follow_symlinks=False)
    need(stat.S_ISREG(expected.st_mode) and 0 < expected.st_size <= MAX_BROWSER_FILE,
         f"invalid browser stage file: {name}")
    flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC
    with os.fdopen(os.open(name, flags, dir_fd=held), "rb") as inp:
        observed = os.fstat(inp.fileno())
        need(os.path.samestat(expected, observed) and stat.S_ISREG(observed.st_mode)
             and observed.st_size == expected.st_size, f"browser stage file changed: {name}")
        body = inp.read(MAX_BROWSER_FILE + 1)
    need(len(body) == expected.st_size, f"browser stage file changed: {name}")
    return body


def prepare_browser_stage(source, approved, copied):
    # Admit the same receipt bytes that Cargo later verifies with the supplied
    # whole-stage pin. Merely checking a receipt in the original path would not
    # bind this source check to the bytes selected by the embedding build step.
    with directory(approved) as held:
        cursor = os.open(".", os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC, dir_fd=held)
        try:
            need(set(os.listdir(cursor)) == {*BROWSER_ASSETS, BROWSER_RECEIPT},
                 "expected exactly four browser resources and their provenance")
        finally:
            os.close(cursor)
        raw = read_browser_file(held, BROWSER_RECEIPT)
        receipt = json.loads(raw.decode("utf-8"))
        need(isinstance(receipt, dict) and receipt.get("format") == 1
             and receipt.get("target") == BROWSER_TARGET, "invalid browser provenance")
        need(receipt.get("recipe") == browser_recipe_inputs(source)
             and receipt.get("source") == browser_inputs(source / "browser"),
             "restage browser resources from this exact release source and recipe")
        copied.mkdir(mode=0o700)
        records = {}
        for name in BROWSER_ASSETS:
            body = read_browser_file(held, name)
            records[name] = {"bytes": len(body), "sha256": hashlib.sha256(body).hexdigest()}
            (copied / name).write_bytes(body)
        need(receipt.get("assets") == records, "browser provenance does not match the four resources")
        (copied / BROWSER_RECEIPT).write_bytes(raw)
        require_name(approved, held)


def qualify(path, env):
    """Check bytes, never test the image by starting ZXS or an ELF loader."""
    observed = path.lstat()
    need(stat.S_ISREG(observed.st_mode) and 64 <= observed.st_size <= MAX_IMAGE,
         "not one bounded ELF file")
    with path.open("rb") as image:
        need(image.read(7) == b"\x7fELF\x02\x01\x01", "not ELF64 little-endian")
    info = text(["/usr/bin/readelf", "-hlWd", path], env)
    need(re.search(r"^\s*Machine:\s+Advanced Micro Devices X86-64\s*$", info, re.M),
         "not an x86-64 program")
    need(re.search(r"^\s*Type:\s+DYN\b", info, re.M) and
         re.search(r"\(FLAGS_1\).*\bPIE\b", info), "not a static PIE program")
    need(not re.search(r"^\s*INTERP\b|\((?:NEEDED|RPATH|RUNPATH|FILTER|AUXILIARY|AUDIT|DEPAUDIT)\)", info, re.M),
         "unexpected ELF interpreter, load or search dependency")
    stacks = [line.split() for line in info.splitlines() if re.match(r"^\s*GNU_STACK\b", line)]
    need(len(stacks) == 1 and len(stacks[0]) >= 8 and not any("E" in f for f in stacks[0][6:-1]),
         "invalid or executable ELF stack")
    return observed.st_size, sha256(path)


def approved_browser_path(raw, pin):
    need(re.fullmatch(r"[0-9a-f]{64}", pin), "expected one independently reviewed SHA-256 browser stage pin")
    candidate = Path(raw)
    need(raw.isprintable() and candidate.is_absolute() and str(candidate.resolve(strict=True)) == raw,
         "browser stage must be one canonical absolute directory")
    return candidate


def build(source, bindir, browser_assets, browser_pin):
    host = os.uname()
    need((host.sysname, host.machine) == ("Linux", "x86_64"), "the primary build host is Linux x86-64")
    need(os.getuid() == os.geteuid() != 0, "build in a private, unprivileged worktree")
    cargo, rustc, rustdoc = (bindir / f for f in ("cargo", "rustc", "rustdoc"))
    need(all(f.is_file() and os.access(f, os.X_OK) for f in (cargo, rustc, rustdoc)),
         "pass the reviewed local Rust toolchain's bin directory")
    for parent in (source, *source.parents):
        for name in ("config", "config.toml"):
            need(not os.path.lexists(parent / ".cargo" / name),
                 f"ambient Cargo configuration is not a release input: {parent / '.cargo' / name}")
    approved = approved_browser_path(browser_assets, browser_pin)
    scratch = source / "target"
    with directory(source) as root, directory("target", root, create=True) as target, \
         directory("standalone", target, create=True) as output:
        try:
            os.stat("zxs", dir_fd=output, follow_symlinks=False)
        except FileNotFoundError:
            pass
        else:
            raise ValueError(f"refusing to replace {scratch / 'standalone' / 'zxs'}")
        compile_into(source, scratch, (root, target, output), cargo, rustc, rustdoc, approved, browser_pin)


def compile_into(source, scratch, originals, cargo, rustc, rustdoc, approved, browser_pin):
    root, target, output = originals
    require_name(source, root)
    require_name("target", target, root)
    with tempfile.TemporaryDirectory(prefix=".zxs-build-", dir=scratch) as tmp:
        stage = Path(tmp)
        home, cargo_home, temp = (stage / f for f in ("home", "cargo", "tmp"))
        for p in (home, cargo_home, temp):
            p.mkdir(mode=0o700)
        dest = stage / "target"
        browser = stage / "browser-assets"
        prepare_browser_stage(source, approved, browser)
        # No inherited Cargo/Rust/C flags, wrappers, user config or source
        # credentials. This uses the local reviewed tools; no fetch/install.
        env = {
            "PATH": "/usr/bin:/bin", "HOME": str(home), "CARGO_HOME": str(cargo_home),
            "TMPDIR": str(temp), "LC_ALL": "C", "TZ": "UTC", "SOURCE_DATE_EPOCH": "0",
            "RUSTC": str(rustc), "RUSTDOC": str(rustdoc),
            "CC": "/usr/bin/gcc", "AR": "/usr/bin/ar", "CARGO_TARGET_DIR": str(dest),
            "CARGO_BUILD_JOBS": "2", "CARGO_INCREMENTAL": "0",
            "ZXS_BROWSER_ASSETS": str(browser), "ZXS_BROWSER_ASSET_PIN": browser_pin,
            "CARGO_ENCODED_RUSTFLAGS": "\x1f".join((
                "-C", "target-feature=+crt-static", "-C", "linker=/usr/bin/gcc",
                f"--remap-path-prefix={source}=/zxs/source",
                f"--remap-path-prefix={stage}=/zxs/build",
            )),
        }
        version = text([rustc, "-vV"], env)
        for line in (f"release: {RUST}", f"commit-hash: {RUST_COMMIT}", f"host: {TARGET}"):
            need(line in version.splitlines(), "unsupported local Rust toolchain")
        cargo_version = text([cargo, "-V"], env).strip()
        need(cargo_version.startswith(f"cargo {RUST} "), "unsupported Cargo version")
        gcc = env["CC"]
        need(text([gcc, "-dumpmachine"], env).strip() == "x86_64-linux-gnu", "unsupported C target")
        # Capture the actual C closure used by the current builder rather
        # than pretending the Rust commit pins distro CRT/libgcc patches.
        c_versions = [text(cmd, env).splitlines()[0] for cmd in (
            [gcc, "--version"], ["/usr/bin/ar", "--version"], ["/usr/bin/ld", "--version"],
            ["/usr/bin/getconf", "GNU_LIBC_VERSION"])]
        c_archives = []
        for n in ("rcrt1.o", "crti.o", "crtbeginS.o", "crtendS.o", "crtn.o", "libc.a",
                  "libc_nonshared.a", "libgcc.a", "libgcc_eh.a", "libpthread.a"):
            p = Path(text([gcc, f"-print-file-name={n}"], env).strip())
            need(p.is_absolute() and p.is_file(), f"missing local C toolchain object: {n}")
            c_archives.append((n, sha256(p)))
        subprocess.run([cargo, "build", "--locked", "--offline", "--release", "--target", TARGET,
                        "--bin", "zxs",
                        "--manifest-path", source / "Cargo.toml"],
                       check=True, cwd=source, env=env)
        built = dest / TARGET / "release" / "zxs"
        record = qualify(built, env)
        # Cargo may hard link into its own deps tree. Stage a new one-link
        # file and claim the sole output name without following/overwriting.
        ready = stage / "qualified-zxs"
        with built.open("rb") as inp, ready.open("xb") as out:
            shutil.copyfileobj(inp, out)
            out.flush()
            os.fchmod(out.fileno(), 0o755)
            os.fsync(out.fileno())
        need(qualify(ready, env) == record, "compiled image changed across export")
        require_name(source, root)
        require_name("target", target, root)
        require_name("standalone", output, target)
        os.link(ready, "zxs", dst_dir_fd=output, follow_symlinks=False)
        ready.unlink()
    final = scratch / "standalone" / "zxs"
    print(f"{final}: {record[0]} bytes; SHA256 {record[1]}")
    print(f"Browser asset stage: SHA256 {browser_pin}")
    print(version.strip())
    print(cargo_version)
    print(*c_versions, sep="\n")
    for n, checksum in c_archives:
        print(f"{n}: SHA256 {checksum}")
    print("Built and qualified bytes only; no service, site approval, or deployment.")


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("toolchain_bin", type=Path, help="local, reviewed Rust 1.100.0 bin directory")
    ap.add_argument("--browser-assets", required=True, help="absolute directory of the reviewed browser stage")
    ap.add_argument("--browser-asset-pin", required=True, help="independently reviewed browser stage SHA-256")
    args = ap.parse_args()
    try:
        build(Path(__file__).resolve().parent, args.toolchain_bin.resolve(strict=True),
              args.browser_assets, args.browser_asset_pin)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        ap.exit(1, f"ZXS build refused: {error}\n")


if __name__ == "__main__":
    main()
