"""Build one optional raw Helper from exact, separately approved ZXS bytes.

This recipe never downloads, starts, installs, adopts or runs either product.
Run as an ordinary non-root user inside a trusted private source workspace.
"""
import argparse
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import struct
import subprocess
import tempfile

MAX_IMAGE = 64 << 20
MAX_HELPER = 64 << 20
RUST_REV = "0ef1bc386059596dada8fb65b50425aab2ea637a"
EXPECTED_HOST = "x86_64-unknown-linux-gnu"
SQLITE_CONFIG_SHA = "9523c1d40a68e848d7ab68b258828f7f5ccb38e6e63e740ed8369e93776fd29e"
# Dynamic dependencies, search paths, audit, config, auxiliary and filter.
NO_RUNTIME = {1, 15, 29, 0x6ffffefa, 0x6ffffefb, 0x6ffffefc, 0x7ffffffd, 0x7fffffff}


def need(ok, message):
    if not ok:
        raise ValueError(message)


def qualified_path(text):
    p = Path(text)
    need(p.is_absolute() and str(p) == text and p.resolve(strict=True) == p,
         "use a real canonical absolute input, without aliases or symlinks")
    return p


def stamp(fd):
    s = os.fstat(fd)
    return s.st_dev, s.st_ino, s.st_mode, s.st_uid, s.st_gid, s.st_nlink, s.st_size, s.st_ctime_ns, s.st_mtime_ns


def admitted_bytes(path, limit, single_link=True, parent=None):
    pin = os.open(path, os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=parent)
    try:
        before = stamp(pin)
        need(stat.S_ISREG(before[2]) and before[2] & 0o6022 == 0
             and before[3] in {0, os.getuid()} and before[5] >= 1
             and (not single_link or before[5] == 1) and 1 <= before[6] <= limit,
             "not a protected original regular release input")
        need(os.readlink("/proc/self") == str(os.getpid()), "a genuine current Linux procfs is required")
        fd = os.open(f"/proc/self/fd/{pin}", os.O_RDONLY | os.O_NONBLOCK | os.O_CLOEXEC)
        with os.fdopen(fd, "rb") as read:
            need(stamp(read.fileno()) == before, "release input switched")
            b = read.read(before[6] + 1)
            need(len(b) == before[6] and stamp(read.fileno()) == stamp(pin) == before,
                 "release input changed or was incomplete")
        return b
    finally:
        os.close(pin)


def digest(b):
    return hashlib.sha256(b).hexdigest()


def elf(data):
    need(len(data) >= 64 and data.startswith(b"\x7fELF\x02\x01\x01"), "need a little-endian ELF64")
    kind, machine, version, entry, phoff = struct.unpack_from("<HHIQQ", data, 16)
    ehsize, phsize, count = struct.unpack_from("<HHH", data, 52)
    need(kind in {2, 3} and machine == 62 and version == 1 and entry > 0
         and ehsize == 64 and phsize == 56 and 1 <= count <= 128 and phoff >= 64
         and phoff + 56 * count <= len(data), "unsupported x86-64 executable headers")
    rows = [struct.unpack_from("<IIQQQQQQ", data, phoff + i * 56) for i in range(count)]
    load = [r for r in rows if r[0] == 1]
    for _, flags, off, addr, _, size, memory, _ in load:
        need(off <= len(data) and off + size <= len(data) and memory >= size
             and addr + memory < 1 << 64, "malformed file-backed ELF region")
    need(any(r[1] & 7 == 5 and r[3] <= entry < r[3] + r[5] for r in load),
         "entry has no file-backed read-only executable region")
    return kind, rows, load


def qualified_zxs(data):
    kind, rows, load = elf(data)
    stacks = [r for r in rows if r[0] == 0x6474e551]
    need(kind == 3 and not any(r[0] == 3 for r in rows) and len(stacks) == 1 and stacks[0][1] & 1 == 0,
         "ZXS is not one standalone static PIE with a non-executable stack")
    dynamic = [r for r in rows if r[0] == 2]
    need(len(dynamic) == 1, "need one original PIE metadata range")
    _, _, off, addr, _, size, memory, _ = dynamic[0]
    need(16 <= size <= 128 << 10 and size % 16 == 0 and size <= memory and off + size <= len(data)
         and any(0 <= addr - r[3] and off == r[2] + addr - r[3]
                 and addr - r[3] + size <= r[5] for r in load), "unmapped or oversized PIE metadata")
    ended = pie = False
    for p in range(off, off + size, 16):
        tag, val = struct.unpack_from("<QQ", data, p)
        need((not ended or tag == 0) and tag not in NO_RUNTIME, "external loader tag or trailing dynamic records")
        ended |= tag == 0
        pie |= tag == 0x6ffffffb and bool(val & 0x08000000)
    need(ended and pie, "not a terminated standalone PIE")


def verify_embedding(helper, zxs):
    _, _, load = elf(helper)
    start = helper.find(zxs)
    need(start >= 0 and helper.find(zxs, start + 1) < 0, "need exactly one complete unchanged ZXS occurrence")
    end = start + len(zxs)
    sharing = [r for r in load if r[5] and r[2] < end and start < r[2] + r[5]]
    need(len(sharing) == 1 and sharing[0][1] == 4 and sharing[0][2] <= start
         and end <= sharing[0][2] + sharing[0][5],
         "exact ZXS payload intersects another load or not entirely in one read-only loadable range")
    return start


def owned_dir(path, device):
    s = path.lstat()
    need(stat.S_ISDIR(s.st_mode) and s.st_uid == os.getuid() and s.st_mode & 0o022 == 0 and s.st_dev == device,
         "source and output must be the same user-owned private local workspace")


def mount_id(fd):
    with open(f"/proc/self/fdinfo/{fd}", "rb") as info:
        raw = info.read(4097)
    rows = re.findall(rb"^mnt_id:[ \t]+([1-9][0-9]*)$", raw, re.M)
    need(len(raw) <= 4096 and raw.endswith(b"\n") and len(rows) == 1,
         "cannot establish the actual held local build mount")
    return int(rows[0])


@contextmanager
def kept_directory(path, parent=None, create=False):
    try:
        fd = os.open(path, os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=parent)
    except FileNotFoundError:
        if not create:
            raise
        try:
            os.mkdir(path, mode=0o700, dir_fd=parent)
        except FileExistsError:
            pass
        fd = os.open(path, os.O_PATH | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=parent)
    try:
        m = os.fstat(fd)
        need(stat.S_ISDIR(m.st_mode) and m.st_uid == os.geteuid() and m.st_mode & 0o022 == 0,
             "build directory must remain privately builder-owned")
        if parent is not None:
            need(mount_id(fd) == mount_id(parent), "refusing a mounted or redirected build subdirectory")
        yield fd
    finally:
        os.close(fd)


def check_name(path, held, parent=None):
    with kept_directory(path, parent) as current:
        need(os.path.samestat(os.fstat(current), os.fstat(held)) and mount_id(current) == mount_id(held),
             "the original build directory name was replaced")


def publish_bytes(staged, output, help_bytes, check_layout):
    file_name = "qualified-helper"
    fd = os.open(file_name, os.O_RDWR | os.O_NOFOLLOW | os.O_CLOEXEC | os.O_CREAT | os.O_EXCL,
                 0o600, dir_fd=staged)
    with os.fdopen(fd, "w+b") as ready:
        ready.write(help_bytes)
        ready.flush()
        os.fchmod(ready.fileno(), 0o755)
        os.fsync(ready.fileno())
        need(admitted_bytes(file_name, MAX_HELPER, parent=staged) == help_bytes, "release bytes changed before local export")
        check_layout()
        os.link(file_name, "helper", src_dir_fd=staged, dst_dir_fd=output, follow_symlinks=False)
        try:
            observed = os.stat("helper", dir_fd=output, follow_symlinks=False)
            src = os.stat(file_name, dir_fd=staged, follow_symlinks=False)
            need(os.path.samestat(observed, os.fstat(ready.fileno())) and os.path.samestat(src, observed),
                 "published Helper does not match the same held output")
            os.unlink(file_name, dir_fd=staged)
            outfd = os.open(".", os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=output)
            try:
                need(os.path.samestat(os.fstat(outfd), os.fstat(output)) and mount_id(outfd) == mount_id(output),
                     "cannot sync the original immutable output directory")
                os.fsync(outfd)
            finally:
                os.close(outfd)
            check_layout()
            observed = os.stat("helper", dir_fd=output, follow_symlinks=False)
            need(os.path.samestat(observed, os.fstat(ready.fileno()))
                 and admitted_bytes("helper", MAX_HELPER, parent=output) == help_bytes,
                 "release bytes changed after local export")
        except (OSError, ValueError) as e:
            raise ValueError(f"standalone Helper output may exist; preserve it and inspect the original before retry: {e}") from e


def config_path_is_present(path):
    try:
        path.lstat()
        return True
    except FileNotFoundError:
        return False


def check_cargo_scope(source, device):
    for i, parent in enumerate([source, *source.parents]):
        cfg = parent / ".cargo"
        if i == 0:
            owned_dir(cfg, device)
            need(not config_path_is_present(cfg / "config")
                 and digest(admitted_bytes(cfg / "config.toml", 4096)) == SQLITE_CONFIG_SHA,
                 "only the tracked Helper SQLite build flags are allowed")
        elif config_path_is_present(cfg):
            need(stat.S_ISDIR(cfg.lstat().st_mode), "an ancestor .cargo is not an original directory")
            need(not config_path_is_present(cfg / "config") and not config_path_is_present(cfg / "config.toml"),
                 "an ancestor Cargo config would change this independent build")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rust-bin", required=True, help="trusted local Rust 1.100.0 bin directory")
    parser.add_argument("--offline-registry", required=True, help="verified local Cargo.lock vendor directory")
    parser.add_argument("--zxs-elf", required=True, help="complete qualified standalone ZXS ELF")
    parser.add_argument("--zxs-sha256", required=True, help="independently supplied exact whole ZXS digest")
    parser.add_argument("--zxs-source-tree", required=True, help="record independently approved ZXS source tree SHA-1")
    a = parser.parse_args()
    need(os.getuid() != 0 and os.getuid() == os.geteuid(), "never build from a root or setuid process")
    need(re.fullmatch("[0-9a-f]{64}", a.zxs_sha256) and a.zxs_sha256 != "0" * 64
         and re.fullmatch("[0-9a-f]{40}", a.zxs_source_tree) and a.zxs_source_tree != "0" * 40,
         "the independently approved digest and source identity are mandatory")
    tools = qualified_path(a.rust_bin)
    vendor = qualified_path(a.offline_registry)
    need(tools.is_dir() and vendor.is_dir(), "toolchain and verified offline vendor directories are mandatory")
    rustc, cargo = tools / "rustc", tools / "cargo"
    need(rustc.is_file() and cargo.is_file(), "trusted local rustc and cargo are required")

    zxs = admitted_bytes(qualified_path(a.zxs_elf), MAX_IMAGE)
    need(digest(zxs) == a.zxs_sha256, "different ZXS bytes than the independently approved input")
    qualified_zxs(zxs)
    source = Path(__file__).resolve().parent
    device = source.stat().st_dev
    owned_dir(source, device)
    check_cargo_scope(source, device)
    with kept_directory(source) as root, kept_directory("target", root, create=True) as target, \
         kept_directory("standalone", target, create=True) as output:
        try:
            os.stat("helper", dir_fd=output, follow_symlinks=False)
        except FileNotFoundError:
            pass
        else:
            raise ValueError("standalone Helper output already exists; use a new private worktree")
        compile_into(source, (root, target, output), rustc, cargo, vendor, zxs, a)


def compile_into(source, held, rustc, cargo, vendor, zxs, a):
    root, target, output = held
    scratch = source / "target"
    check_name(source, root)
    check_name("target", target, root)
    with tempfile.TemporaryDirectory(prefix=".embedded-helper-", dir=scratch) as name:
        work = Path(name)
        with kept_directory(work.name, target) as staged:
            def layout():
                check_name(source, root)
                check_name("target", target, root)
                check_name("standalone", output, target)
                check_name(work.name, staged, target)
            layout()
            cargo_home, tmp = work / "cargo", work / "tmp"
            cargo_home.mkdir(mode=0o700)
            tmp.mkdir(mode=0o700)
            (cargo_home / "config.toml").write_text(
                '[source.crates-io]\nreplace-with = "approved-local"\n'
                '[source.approved-local]\ndirectory = ' + json.dumps(str(vendor), ensure_ascii=False) + '\n',
                encoding="utf-8")
            asset = work / "zxs.elf"
            asset.write_bytes(zxs)
            asset.chmod(0o400)
            need(admitted_bytes("zxs.elf", MAX_IMAGE, parent=staged) == zxs, "private build snapshot is not exact")
            env = {"PATH": "/usr/bin:/bin", "LANG": "C", "LC_ALL": "C", "TZ": "UTC",
                   "HOME": str(work), "CARGO_HOME": str(cargo_home), "RUSTC": str(rustc),
                   "CARGO_TARGET_DIR": str(work / "build"), "SOURCE_DATE_EPOCH": "0", "TMPDIR": str(tmp),
                   "HELPER_ZXS_PAYLOAD_FILE": str(asset), "HELPER_ZXS_PAYLOAD_SHA256": a.zxs_sha256}
            v = subprocess.run([rustc, "-Vv"], env=env, check=True, capture_output=True, text=True).stdout.splitlines()
            c = subprocess.run([cargo, "-V"], env=env, check=True, capture_output=True, text=True).stdout.strip()
            need("release: 1.100.0" in v and f"commit-hash: {RUST_REV}" in v and f"host: {EXPECTED_HOST}" in v
                 and c.startswith("cargo 1.100.0 "), "unexpected compiler, Cargo revision or host")
            subprocess.run([cargo, "build", "--release", "--locked", "--offline", "--target", EXPECTED_HOST,
                            "--features", "embedded-zxs", "--bin", "helper"], cwd=source, env=env, check=True)
            layout()
            check_cargo_scope(source, os.fstat(root).st_dev)
            product = work / "build" / EXPECTED_HOST / "release" / "helper"
            # Cargo may hardlink its compiled binary to deps. Snapshot actual
            # emitted bytes into our own singly linked release inode before approval.
            helper = admitted_bytes(product, MAX_HELPER, single_link=False)
            offset = verify_embedding(helper, zxs)
            publish_bytes(staged, output, helper, layout)
            print(json.dumps({"output": "target/standalone/helper", "helper_sha256": digest(helper),
                              "helper_bytes": len(helper), "zxs_bytes": len(zxs), "zxs_payload_offset": offset,
                              "zxs_sha256": digest(zxs), "recorded_zxs_source_tree_sha1": a.zxs_source_tree},
                             sort_keys=True))


if __name__ == "__main__":
    main()
