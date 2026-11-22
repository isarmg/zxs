"""Prepare fixed offline QA material for the same approved hosted build only."""

import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import subprocess
import sys
import tarfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from guest_smb_move_material import (CLIENT, ELF_MAX, LIB, MEMBER_LIMIT, NFT, PAYLOAD_MAX, SNAPSHOT,
                                     WHEELS, digest_path, manifest, member_kind, need, package_table)
from guest_smb_move import approved_request
from rust_smb_move_ci import object_once, read_regular

WHEEL_URL = {
    "smbprotocol-1.17.0-py3-none-any.whl": "https://files.pythonhosted.org/packages/76/d3/250e13e7b3473a0f441f4d680d511ad30ce432d0180115dec9840d88fe89/",
    "pyspnego-0.12.4-py3-none-any.whl": "https://files.pythonhosted.org/packages/eb/4e/f6c1bb147890f246c3a24ce836be4f6cfc44fd7be08e0a229e3a1c08b560/",
}
NATIVE_TARGET = "x86_64-unknown-linux-gnu"


def checked(cmd, *, cwd, env, output=None, timeout=120):
    subprocess.run(cmd, cwd=cwd, env=env, timeout=timeout, stdin=subprocess.DEVNULL,
                   stdout=subprocess.DEVNULL if output is None else output,
                   stderr=subprocess.DEVNULL, check=True)


def download(url, path, size, approved, *, env):
    need(not os.path.lexists(path))
    checked(("/usr/bin/curl", "--disable", "--fail", "--silent", "--show-error", "--location",
             "--proto", "=https", "--proto-redir", "=https", "--connect-timeout", "15",
             "--max-time", "90", "--max-filesize", str(size), "-o", str(path), url),
            cwd=path.parent, env=env, timeout=110)
    need(path.lstat().st_size == size and digest_path(path, size) == approved)


def add_member(members, root, path, role):
    name = path.relative_to(root).as_posix()
    need(member_kind(name) == role and name not in members and len(members) < MEMBER_LIMIT)
    item = path.lstat()
    need(stat.S_ISREG(item.st_mode) or role == "library" and stat.S_ISLNK(item.st_mode))
    members[name] = path


def runtime(output, suite, approved, build, env):
    members = {}
    for package in package_table(suite):
        name = package["package"]
        deb = build / (name + ".deb")
        download(SNAPSHOT + "/" + package["filename"], deb, package["size"], package["sha256"], env=env)
        root = build / name
        root.mkdir(mode=0o700)
        # Never install a package, run a maintainer script, load a provider,
        # start a Samba daemon, or copy a service/configuration from it.
        checked(("/usr/bin/dpkg-deb", "--extract", str(deb), str(root)), cwd=build, env=env)
        libs = root / LIB
        if libs.is_dir():
            need(libs.resolve() == libs)
            for path in sorted(libs.rglob("*")):
                if member_kind(path.relative_to(root).as_posix()) == "library":
                    add_member(members, root, path, "library")
        if name == "smbclient":
            path = root / CLIENT
            need(digest_path(path, ELF_MAX) == approved["client_elf_sha256"])
            add_member(members, root, path, "program")
        elif name == "nftables":
            add_member(members, root, root / NFT, "program")
    wheels = build / "wheels"
    wheels.mkdir(mode=0o700)
    for name, (size, sha) in WHEELS.items():
        wheel = wheels / name
        download(WHEEL_URL[name] + name, wheel, size, sha, env=env)
        add_member(members, build, wheel, "wheel")
    total = 0
    with tarfile.open(output, "x:", format=tarfile.USTAR_FORMAT) as out:
        for name, origin in sorted(members.items()):
            info = tarfile.TarInfo(name)
            original = origin.lstat()
            if stat.S_ISLNK(original.st_mode):
                target = origin.readlink()
                resolved = str(PurePosixPath(name).parent / target.as_posix())
                need(len(target.parts) == 1 and target.name not in (".", "..") and resolved != name
                     and resolved in members and stat.S_ISREG(members[resolved].lstat().st_mode))
                info.type, info.linkname = tarfile.SYMTYPE, target.as_posix()
                out.addfile(info)
            else:
                need(0 < original.st_size <= 32 * 1024 * 1024)
                info.size = original.st_size
                total += info.size
                need(total <= PAYLOAD_MAX)
                info.mode = 0o755 if member_kind(name) == "program" else 0o644
                with origin.open("rb") as body:
                    need(os.path.samestat(os.fstat(body.fileno()), original))
                    out.addfile(info, body)


def history(output, work, env, pair):
    # That pair producer reconstructed the exact same approved commit, staged
    # the official Rust distribution, and checked the actual reviewed browser
    # stage. Build but never execute the internal read-only libtest witness here.
    staged = work / "pair-build" / "source" / "zxs"
    bindir = work / "pair-build" / "rust" / "bin"
    build = work / "offline-build"
    target = build / "history-target"
    cargo = build / "history-cargo"
    cargo.mkdir(mode=0o700)
    assets = staged / "target" / "browser-assets"
    raw_pin = (staged / "browser-stage.sha256").read_bytes()
    need(re.fullmatch(rb"[0-9a-f]{64}\n", raw_pin) and raw_pin[:-1].decode("ascii") == pair["browser_stage_sha256"])
    local = dict(env, CARGO_HOME=str(cargo), CARGO_TARGET_DIR=str(target), CARGO_BUILD_JOBS="2",
                 CARGO_INCREMENTAL="0", RUSTC=str(bindir / "rustc"), RUSTDOC=str(bindir / "rustdoc"),
                 CC="/usr/bin/gcc", AR="/usr/bin/ar", CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER="/usr/bin/gcc",
                 ZXS_BROWSER_ASSETS=str(assets), ZXS_BROWSER_ASSET_PIN=pair["browser_stage_sha256"])
    messages = build / "history-cargo.jsonl"
    with messages.open("xb") as stream:
        checked((str(bindir / "cargo"), "test", "--release", "--locked", "--offline", "--no-run", "--lib",
                 "--target", NATIVE_TARGET, "--message-format=json"), cwd=staged, env=local, output=stream, timeout=900)
    need(0 < messages.lstat().st_size <= 4 * 1024 * 1024)
    selected = []
    with messages.open("rb") as stream:
        for raw in stream:
            record = json.loads(raw)
            if (record.get("reason") == "compiler-artifact" and record.get("executable")
                    and record["target"]["name"] == "storage_server" and record["profile"]["test"] is True
                    and Path(record["target"]["src_path"]) == staged / "src" / "lib.rs"):
                selected.append(Path(record["executable"]))
        need(record.get("reason") == "build-finished" and record.get("success") is True)
    need(len(selected) == 1)
    executable, = selected
    # Cargo's internal artifact folders are not a contract. The exact JSON
    # source/target/profile above and the same exclusive build root are.
    need(target.resolve(strict=True) == target and executable.resolve(strict=True) == executable
         and executable.is_relative_to(target) and re.fullmatch(r"storage_server-[0-9a-f]{16}", executable.name))
    approved_executable = digest_path(executable, ELF_MAX)
    with executable.open("rb") as stream:
        need(stream.read(7) == b"\x7fELF\x02\x01\x01")
    with executable.open("rb") as body, output.open("xb") as target_file:
        shutil.copyfileobj(body, target_file)
    need(digest_path(output, ELF_MAX) == approved_executable)


def main(argv):
    need(len(argv) == 2 and os.getuid() == os.geteuid() != 0)
    source, work = map(Path, argv)
    suite = source / "zxs/tests/acceptance"
    need(source.is_absolute() and work.is_absolute() and work.parent == source.parent
         and suite / "guest_smb_move_payload.py" == Path(__file__).resolve())
    request = approved_request(work)
    approved = manifest(suite, request)
    pair = json.loads(read_regular(work / "pair" / "receipt.json"), object_pairs_hook=object_once)
    anchors = ("source", "challenge", "guest_inputs_sha256")
    need(pair["profile"] == "SMB-MOVE-PAIR-1" and all(pair[k] == request[k] for k in anchors))
    output, build = work / "offline", work / "offline-build"
    need(not os.path.lexists(output))
    os.umask(0o077)
    build.mkdir(mode=0o700)
    pending = build / "publish"
    pending.mkdir(mode=0o700)
    env = {"LC_ALL": "C", "TZ": "UTC", "PATH": "/usr/bin:/bin", "HOME": str(build), "TMPDIR": str(build)}
    runtime(pending / "runtime", suite, approved, build, env)
    history(pending / "history", work, env, pair)
    receipt = {"profile": "SMB-MOVE-OFFLINE-1", **{k: request[k] for k in anchors},
               "runtime_sha256": digest_path(pending / "runtime", PAYLOAD_MAX),
               "history_sha256": digest_path(pending / "history", ELF_MAX)}
    with (pending / "receipt.json").open("x", encoding="ascii") as stream:
        json.dump(receipt, stream, sort_keys=True)
        stream.write("\n")
    need(not os.path.lexists(output))
    pending.rename(output)


if __name__ == "__main__":
    main(sys.argv[1:])
