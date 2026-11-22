"""Build and seal an independently approved pair for one manual Linux guest job."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import stat
from struct import unpack_from
import subprocess
from tempfile import TemporaryDirectory
import tomllib

TARGET = "x86_64-unknown-linux-gnu"
RUST = "1.100.0"
HELPER_BUILD_PATH = "/helper/build"
HELPER_FEATURE = "private-fresh-install"  # The one exact fresh deployment used by both acceptance and measurement.
SCHEMA_SHA256 = "5cf057fe1df72bd606ad47f11beec7747779a67206b781bab89d2aeeaf8c8495"
RUST_RELEASE = "https://static.rust-lang.org/dist/2026-11-12"
# Same distribution archives as the Linux paired build receipts workflow.
ARCHIVES = (
    ("rustc", TARGET, "rustc", "f1d0b1dec81dab6f523cd098507763487b0d4bd7fd2c87582f154128719be03b"),
    ("cargo", TARGET, "cargo", "7e29ce07d7c6e92d1826870575ba27d57fff6cb3e754aa57d99099cb88fab35d"),
    ("rust-std", TARGET, "rust-std-x86_64-unknown-linux-gnu",
     "4591b6486594ded52ecd6135db95bf9355bbd3ca7a380888b141528bbcf9a5d4"),
    ("rust-std", "wasm32-unknown-unknown", None,
     "19b7954d650fc0d22767eceb76b8e03d07ce0d6a9f4234f33c32a17ee8d6d7b9"),
)
RESOURCES = ("boot.js", "folder.css", "zxs_browser.js", "zxs_browser_bg.wasm")
MAX_ELF = 64 * 1024 * 1024


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def hex_id(value, length):
    return isinstance(value, str) and re.fullmatch(f"[0-9a-f]{{{length}}}", value) and value != "0" * length


def sha256(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def private_directory(path):
    require(path.is_absolute() and path.resolve(strict=True) == path, "noncanonical private directory")
    info = path.stat()
    require(stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid() and stat.S_IMODE(info.st_mode) == 0o700,
            "pair paths must be exclusive private directories")
    return path


def bounded_read(path, maximum, minimum=1):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
    with os.fdopen(fd, "rb") as stream:
        info = os.fstat(stream.fileno())
        require(stat.S_ISREG(info.st_mode) and minimum <= info.st_size <= maximum,
                "expected one bounded regular input")
        raw = stream.read(maximum + 1)
        require(len(raw) == info.st_size, "incomplete input")
        return raw


def command(*args, env, cwd, capture=False):
    return subprocess.run(args, cwd=cwd, env=env, check=True, stdin=subprocess.DEVNULL,
                          stdout=subprocess.PIPE if capture else None).stdout


def stage_source(repo, commit, build, env):
    git = ("/usr/bin/git", "-C", repo, "-c", "core.hooksPath=/dev/null")
    env = dict(env, GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL="/dev/null", GIT_TERMINAL_PROMPT="0",
               GIT_NO_REPLACE_OBJECTS="1")

    def read_git(*args):
        return command(*git, *args, env=env, cwd=build, capture=True).decode("ascii").strip()

    require(read_git("rev-parse", "--verify", "FETCH_HEAD^{commit}") == commit, "paired source differs from approval")
    trees = {key: read_git("rev-parse", value) for key, value in (
        ("root", f"{commit}^{{tree}}"), ("zxs", f"{commit}:zxs"), ("helper", f"{commit}:helper"))}
    require(all(hex_id(oid, 40) for oid in trees.values()), "unexpected source tree identifiers")
    records = command(*git, "ls-tree", "-rz", commit, "--", "helper", "zxs", env=env, cwd=build, capture=True)
    require(records and records.endswith(b"\0")
            and all(row.startswith((b"100644 blob ", b"100755 blob ")) for row in records[:-1].split(b"\0")),
            "paired source must consist of ordinary tracked files")
    dest = build / "source"
    dest.mkdir(mode=0o700)
    archive = build / "source.tar"
    command(*git, "archive", "--format=tar", "-o", archive, commit, "--", "helper", "zxs", env=env, cwd=build)
    command("/usr/bin/tar", "-xf", archive, "-C", dest, "--no-same-owner", "--no-same-permissions",
            env=env, cwd=build)
    archive.unlink()
    return dest, trees


def stage_rust(build, env):
    bindir = build / "rust"
    bindir.mkdir(mode=0o700)
    recorded = {}
    for component, target, member, expected in ARCHIVES:
        name = f"{component}-{RUST}-{target}"
        archive = build / f"{name}.tar.xz"
        command("/usr/bin/curl", "--disable", "--fail", "--silent", "--show-error", "--location",
                "--proto", "=https", "--proto-redir", "=https", "--connect-timeout", "15",
                "--max-time", "120", "--retry", "2", "--max-filesize", "120000000",
                f"{RUST_RELEASE}/{archive.name}", "-o", archive, env=env, cwd=build)
        require(sha256(archive) == expected, "a Rust distribution archive differs from its reviewed pin")
        recorded[archive.name] = expected
        if member is not None:
            command("/usr/bin/tar", "-xJf", archive, "-C", bindir, "--no-same-owner", "--no-same-permissions",
                    "--strip-components=2", f"--exclude={name}/{member}/manifest.in", f"{name}/{member}",
                    env=env, cwd=build)
        else:
            wasm_std = archive
    return bindir / "bin", wasm_std, recorded


def embedded_resources(image, asset_dir):
    # Recheck the original readonly-load proof from the Linux paired build.
    page = 4096
    start, = unpack_from("<Q", image, 32)
    width, count = unpack_from("<HH", image, 54)
    require(width == 56 and 1 <= count <= 128 and start + width * count <= len(image), "bad ZXS ELF headers")
    loads = [row for i in range(count) if (row := unpack_from("<IIQQQQQQ", image, start + width * i))[0] == 1]
    for _, _, offset, address, _, length, memory, _ in loads:
        require(offset + length <= len(image) and length <= memory and address + memory < 1 << 64
                and (not length or offset % page == address % page), "invalid ZXS load mapping")
    facts = {}
    for name in RESOURCES:
        raw = (asset_dir / name).read_bytes()  # Already checked against the reviewed stage by build_release.py.
        at = image.find(raw)
        end = at + len(raw)
        require(raw and at >= 0 and image.find(raw, at + 1) == -1, "missing or repeated embedded resource")
        owners = [row for row in loads if row[5] and row[2] < end and at < row[2] + row[5]]
        require(len(owners) == 1, "embedded resource crosses ELF mappings")
        owner, = owners
        require(owner[1] == 4 and owner[2] <= at and end <= owner[2] + owner[5], "non-readonly embedded resource")
        address = owner[3] + at - owner[2]
        require(all(row is owner or not row[6] or address + len(raw) <= row[3] // page * page
                    or (row[3] + row[6] + page - 1) // page * page <= address for row in loads),
                "embedded resource aliases another load page")
        facts[name] = {"bytes": len(raw), "sha256": hashlib.sha256(raw).hexdigest()}
    return facts


def helper_elf(path, env, cwd):
    info = command("/usr/bin/readelf", "-hlWdV", path, env=env, cwd=cwd, capture=True).decode("ascii")
    require(re.search(r"^\s*Class:\s+ELF64$", info, re.M)
            and re.search(r"^\s*Machine:\s+Advanced Micro Devices X86-64$", info, re.M),
            "unexpected Helper ELF target")
    stacks = [line.split() for line in info.splitlines() if re.match(r"^\s*GNU_STACK\b", line)]
    require(len(stacks) == 1 and len(stacks[0]) >= 8 and not any("E" in f for f in stacks[0][6:-1]),
            "missing or executable Helper stack")
    loaders = re.findall(r"\[Requesting program interpreter: ([^\]]+)\]", info)
    needed = set(re.findall(r"\(NEEDED\).*\[([^\]]+)\]", info))
    require(loaders == ["/lib64/ld-linux-x86-64.so.2"]
            and "libc.so.6" in needed and needed <= {"libgcc_s.so.1", "libc.so.6", "ld-linux-x86-64.so.2"}
            and not re.search(r"\((?:RPATH|RUNPATH|FILTER|AUXILIARY|AUDIT|DEPAUDIT)\)", info),
            "unexpected Helper dynamic loader or dependencies")
    return {"interpreter": loaders[0], "needed": sorted(needed), "readelf_sha256": hashlib.sha256(info.encode()).hexdigest()}


def build_helper(source, build, bindir, env):
    # Use the same official locked registry and sanitized native build as the paired workflow.
    lock = tomllib.loads((source / "Cargo.lock").read_text("utf-8"))
    require(all("source" not in entry or entry["source"] == "registry+https://github.com/rust-lang/crates.io-index"
                and hex_id(entry.get("checksum"), 64) for entry in lock["package"]), "unexpected Helper registry input")
    require(not any(os.path.lexists(parent / ".cargo" / name) for parent in (source, *source.parents)
                    for name in ("config", "config.toml")), "ambient Helper Cargo configuration")
    cargo_home, target = build / "cargo", build / "target"
    cargo_home.mkdir(mode=0o700)
    env = dict(env, CARGO_HOME=str(cargo_home), CARGO_TARGET_DIR=str(target), CARGO_BUILD_JOBS="2",
               CARGO_INCREMENTAL="0", RUSTC=str(bindir / "rustc"), RUSTDOC=str(bindir / "rustdoc"),
               CC="/usr/bin/gcc", AR="/usr/bin/ar", CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER="/usr/bin/gcc")
    # Temporary runner paths otherwise leak into Rust panic locations (including
    # registered dependencies) and C source strings, making expected SHAs unusable.
    env.update(SOURCE_DATE_EPOCH="0", CARGO_ENCODED_RUSTFLAGS=f"--remap-path-prefix={build}={HELPER_BUILD_PATH}",
               CFLAGS=f"-ffile-prefix-map={build}={HELPER_BUILD_PATH}")
    cargo = bindir / "cargo"
    command(cargo, "fetch", "--locked", "--target", TARGET, env=env, cwd=source)
    command(cargo, "build", "--locked", "--offline", "--release", "--target", TARGET,
            "--bin", "helper", "--features", HELPER_FEATURE, env=env, cwd=source)
    return target / TARGET / "release" / "helper"


def _private_paths(repo, source, work):
    require(os.getuid() == os.geteuid() != 0 and os.uname().sysname == "Linux" and os.uname().machine == "x86_64",
            "expected an unprivileged Linux x86-64 build runner")
    repo, source, work = map(private_directory, (repo, source, work))
    private_directory(work.parent)
    require(repo == source.parent / "repo" and work.parent == source.parent and work not in (repo, source)
            and Path(__file__).resolve() == source / "zxs/tests/acceptance/rust_smb_move_pair.py",
            "pair producer is not running from the approved private checkout layout")
    return repo, source, work


def _build_pair(repo, commit, build, inspect_digest):
    """The exact same compiler and ELF inspections for measurement and acceptance."""
    build.mkdir(mode=0o700)
    env = {"PATH": "/usr/bin:/bin", "LC_ALL": "C", "TZ": "UTC", "HOME": str(build), "TMPDIR": str(build)}
    staged, trees = stage_source(repo, commit, build, env)
    zxs, helper = staged / "zxs", staged / "helper"
    require(sha256(zxs / "src/system/schema.sql") == sha256(helper / "src/sqlite_install/schema.sql") == SCHEMA_SHA256,
            "both products must contain the exact immutable v19 acceptance schema")
    raw_pin = bounded_read(zxs / "browser-stage.sha256", 65)
    require(re.fullmatch(rb"[0-9a-f]{64}\n", raw_pin), "invalid reviewed browser stage pin")
    pin = raw_pin[:-1].decode("ascii")
    locks = {key: sha256(project / "Cargo.lock") for key, project in (("zxs", zxs), ("helper", helper))}
    recipes = {key: sha256(zxs / key) for key in ("build_browser.py", "build_release.py")}
    bindir, wasm_std, archives = stage_rust(build, env)
    tools = {name: sha256(Path("/usr/bin") / name) for name in ("gcc", "ar", "ld", "readelf")}
    tools.update({name: sha256(bindir / name) for name in ("rustc", "cargo", "rustdoc")})
    assets = zxs / "target" / "browser-assets"
    command("/usr/bin/python3", "-B", zxs / "build_browser.py", bindir, "--wasm-std", wasm_std, env=env, cwd=zxs)
    command("/usr/bin/python3", "-B", zxs / "build_release.py", bindir,
            "--browser-assets", assets, "--browser-asset-pin", pin, env=env, cwd=zxs)
    server_raw = bounded_read(zxs / "target" / "standalone" / "zxs", MAX_ELF, 64)
    server_sha = hashlib.sha256(server_raw).hexdigest()
    inspect_digest("server_sha256", server_sha)
    embedded = embedded_resources(server_raw, assets)
    helper_path = build_helper(helper, build, bindir, env)
    helper_raw = bounded_read(helper_path, MAX_ELF, 64)
    helper_sha = hashlib.sha256(helper_raw).hexdigest()
    inspect_digest("helper_sha256", helper_sha)
    properties = helper_elf(helper_path, env, build)
    return (server_raw, helper_raw), {
        "source": commit, "trees": trees, "target": TARGET,
        "locks_sha256": locks, "recipes_sha256": recipes, "rust_release": RUST, "rust_archives_sha256": archives,
        "tools_sha256": tools, "browser_stage_sha256": pin, "embedded": embedded,
        "server_sha256": server_sha, "server_bytes": len(server_raw),
        "helper_sha256": helper_sha, "helper_bytes": len(helper_raw),
        "helper_feature": HELPER_FEATURE, "helper_elf": properties,
        "helper_build_path_remap": HELPER_BUILD_PATH, "source_date_epoch": 0,
    }


def measure(repo, source, work, commit):
    """Produce only compile evidence, never a handoff or a source of approval."""
    repo, _, work = _private_paths(repo, source, work)
    require(hex_id(commit, 40) and not any(work.iterdir()), "expected a pinned commit and empty measurement directory")
    observed = {}
    with TemporaryDirectory(prefix=".pair-measure-", dir=work) as temporary:
        _, facts = _build_pair(repo, commit, Path(temporary) / "pair-build", observed.__setitem__)
    require(observed == {key: facts[key] for key in ("server_sha256", "helper_sha256")}
            and not any(work.iterdir()), "unexpected measurement inputs or outputs")
    with (work / "measurement.json").open("x", encoding="ascii") as stream:
        json.dump(dict(facts, profile="SMB-MOVE-PAIR-MEASUREMENT-1", schema_sha256=SCHEMA_SHA256), stream, sort_keys=True)
        stream.write("\n")


def prepare(repo, source, work):
    repo, _, work = _private_paths(repo, source, work)
    request = json.loads(bounded_read(work / "request.json", 2048))
    require(isinstance(request, dict) and request.get("profile") == "SMB-MOVE-CI-1"
            and hex_id(request.get("source"), 40)
            and all(hex_id(request.get(key), 64) for key in ("server_sha256", "helper_sha256", "guest_inputs_sha256"))
            and hex_id(request.get("challenge"), 32), "invalid paired build approval")
    output = work / "pair"
    require(not os.path.lexists(output), "paired output already exists")

    def require_approved(key, digest):
        product = {"server_sha256": "ZXS", "helper_sha256": "Helper"}[key]
        require(digest == request[key], f"{product} artifact not independently approved")

    build = work / "pair-build"
    (server_raw, helper_raw), facts = _build_pair(repo, request["source"], build, require_approved)

    # Prepare both unchanged files and the last receipt, then expose the pair together.
    publish = build / "pair"
    publish.mkdir(mode=0o700)
    for name, raw in (("zxs", server_raw), ("helper", helper_raw)):
        with (publish / name).open("xb") as stream:
            stream.write(raw)
            os.fchmod(stream.fileno(), 0o555)
        require(sha256(publish / name) == hashlib.sha256(raw).hexdigest(), "paired export changed the bytes")
    receipt = dict(facts, profile="SMB-MOVE-PAIR-1", challenge=request["challenge"],
                   guest_inputs_sha256=request["guest_inputs_sha256"])
    with (publish / "receipt.json").open("x", encoding="ascii") as stream:
        json.dump(receipt, stream, sort_keys=True)
        stream.write("\n")
    require(not os.path.lexists(output), "paired output path changed")
    publish.rename(output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--work", type=Path, required=True)
    args = parser.parse_args()
    os.umask(0o077)
    try:
        prepare(args.repo, args.source, args.work)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Paired build refused: {error}\n")


if __name__ == "__main__":
    main()
