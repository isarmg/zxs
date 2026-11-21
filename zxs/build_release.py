#!/usr/bin/env python3
"""Build the sole Linux x86-64 ZXS ELF from this reviewed standalone source."""
import argparse
from contextlib import contextmanager
import hashlib
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


def build(source, bindir):
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
    scratch = source / "target"
    with directory(source) as root, directory("target", root, create=True) as target, \
         directory("standalone", target, create=True) as output:
        try:
            os.stat("zxs", dir_fd=output, follow_symlinks=False)
        except FileNotFoundError:
            pass
        else:
            raise ValueError(f"refusing to replace {scratch / 'standalone' / 'zxs'}")
        compile_into(source, scratch, (root, target, output), cargo, rustc, rustdoc)


def compile_into(source, scratch, originals, cargo, rustc, rustdoc):
    root, target, output = originals
    require_name(source, root)
    require_name("target", target, root)
    with tempfile.TemporaryDirectory(prefix=".zxs-build-", dir=scratch) as tmp:
        stage = Path(tmp)
        home, cargo_home, temp = (stage / f for f in ("home", "cargo", "tmp"))
        for p in (home, cargo_home, temp):
            p.mkdir(mode=0o700)
        dest = stage / "target"
        # No inherited Cargo/Rust/C flags, wrappers, user config or source
        # credentials. This uses the local reviewed tools; no fetch/install.
        env = {
            "PATH": "/usr/bin:/bin", "HOME": str(home), "CARGO_HOME": str(cargo_home),
            "TMPDIR": str(temp), "LC_ALL": "C", "TZ": "UTC", "SOURCE_DATE_EPOCH": "0",
            "RUSTC": str(rustc), "RUSTDOC": str(rustdoc),
            "CC": "/usr/bin/gcc", "AR": "/usr/bin/ar", "CARGO_TARGET_DIR": str(dest),
            "CARGO_BUILD_JOBS": "2", "CARGO_INCREMENTAL": "0",
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
                        "--bin", "zxs", "--manifest-path", source / "Cargo.toml"],
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
    print(version.strip())
    print(cargo_version)
    print(*c_versions, sep="\n")
    for n, checksum in c_archives:
        print(f"{n}: SHA256 {checksum}")
    print("Built and qualified bytes only; no service, site approval, or deployment.")


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("toolchain_bin", type=Path, help="local, reviewed Rust 1.100.0 bin directory")
    args = ap.parse_args()
    try:
        build(Path(__file__).resolve().parent, args.toolchain_bin.resolve(strict=True))
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        ap.exit(1, f"ZXS build refused: {error}\n")


if __name__ == "__main__":
    main()
