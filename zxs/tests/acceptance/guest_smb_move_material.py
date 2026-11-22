"""Admit only the fixed independently approved offline files in the new guest."""

from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import stat
import tarfile

from rust_smb_move_ci import hex_id, object_once, read_regular

IMAGE_SHA = "f6c630c04d4fad012f2c4ed3a21a2b0c0f814f13f6873568da4c31d8df65c12e"
SNAPSHOT = 'https://snapshot.ubuntu.com/ubuntu/20261118T120000Z'
TEMP = Path("/run/smb-move-fixture")
LIB = PurePosixPath("usr/lib/x86_64-linux-gnu")
PROVIDER_DIR = Path("/usr/lib/local-ops-helper/disk-tools-v1/lib")
PROVIDER_NAMES = {"gcrypt": "libgcrypt.so.20", "gpgerror": "libgpg-error.so.0"}
CLIENT = "usr/bin/smbclient"
NFT = "usr/sbin/nft"
CLIENT_PACKAGE = "b1adf344e27e90c1c851f10ea8a800e001fefaed89f7898b5d514f01b6197d6b"
CLIENT_BANNER = "Version 4.23.6-Ubuntu-4.23.6+dfsg-1ubuntu2.2"
WHEELS = {
    "smbprotocol-1.17.0-py3-none-any.whl": (128208, "bd1abff5417f5af83ca516a64ab8e5acece3dbdcf58d4e5e23e47f5165a77349"),
    "pyspnego-0.12.4-py3-none-any.whl": (131003, "a29a34de1abe9e9b30d9aa1d363f8e49fc159eb7355b1f2ee86ccb3a2ce3bba9"),
}
PACKAGE_LIST = "guest_smb_move_packages.json"
INPUTS = "guest_smb_move_inputs.json"
ELF_MAX = 64 * 1024 * 1024
PAYLOAD_MAX = 128 * 1024 * 1024
MEMBER_LIMIT = 1024


def need(value):
    if not value:
        raise ValueError("unqualified fixed offline guest inputs")


def file_stamp(info):
    # Forward reads may update atime; identity, contents and ownership must not.
    return (info.st_dev, info.st_ino, info.st_mode, info.st_uid, info.st_gid, info.st_nlink,
            info.st_size, info.st_ctime_ns, info.st_mtime_ns)


def digest_path(path, maximum):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
    with os.fdopen(fd, "rb") as held:
        stamp = os.fstat(held.fileno())
        need(stat.S_ISREG(stamp.st_mode) and stamp.st_nlink == 1 and 0 < stamp.st_size <= maximum)
        digest = hashlib.file_digest(held, "sha256").hexdigest()
        need(file_stamp(os.fstat(held.fileno())) == file_stamp(path.lstat()) == file_stamp(stamp))
        return digest


def manifest(directory, request):
    raw = read_regular(directory / INPUTS)
    need(hashlib.sha256(raw).hexdigest() == request["guest_inputs_sha256"])
    value = json.loads(raw, object_pairs_hook=object_once)
    need(set(value) == {"profile", "image_sha256", "packages_sha256", "client_elf_sha256", "providers"}
         and value["profile"] == "SMB-MOVE-INPUTS-1" and value["image_sha256"] == IMAGE_SHA
         and all(hex_id(value[k], 64) for k in ("packages_sha256", "client_elf_sha256"))
         and set(value["providers"]) == set(PROVIDER_NAMES)
         and digest_path(directory / PACKAGE_LIST, 16 * 1024) == value["packages_sha256"])
    for role, provided in value["providers"].items():
        need(set(provided) == {"member", "sha256"} and hex_id(provided["sha256"], 64)
             and isinstance(provided["member"], str)
             and re.fullmatch(re.escape(PROVIDER_NAMES[role]) + r"(?:\.[0-9]{1,4}){1,3}", provided["member"]))
    return value


def package_table(directory):
    root = json.loads(read_regular(directory / PACKAGE_LIST), object_pairs_hook=object_once)
    need(root["snapshot"] == SNAPSHOT and root["signer"] == "F6ECB3762474EDA9D21B7022871920D1991BC93C")
    expected = {"libarchive13t64", "libavahi-client3", "libavahi-common-data", "libavahi-common3", "libicu78",
                "libjansson4", "libldb2", "liblmdb0", "libnftables1", "libnftnl11", "libpopt0", "libsmbclient0",
                "libtalloc2", "libtdb1", "libtevent0t64", "libwbclient0", "libxml2-16", "nftables", "samba-common",
                "samba-libs", "smbclient"}
    table = root["artifacts"]
    need(len(table) == len(expected) and {row["package"] for row in table} == expected)
    for row in table:
        package, filename = row["package"], row["filename"]
        name = PurePosixPath(filename).name
        need(re.fullmatch(r"pool/main/(?:[A-Za-z0-9.+_-]+/){2}[A-Za-z0-9.+_~-]+\.deb", filename)
             and name.startswith(package + "_") and row["architecture"] in ("amd64", "all")
             and name.endswith(f"_{row['architecture']}.deb") and type(row["size"]) is int
             and 0 < row["size"] <= 32 * 1024 * 1024 and hex_id(row["sha256"], 64))
        if package == "smbclient":
            need(row["sha256"] == CLIENT_PACKAGE and row["version"] == "2:4.23.6+dfsg-1ubuntu2.2")
    return table


def member_kind(name):
    path = PurePosixPath(name)
    need(str(path) == name and not path.is_absolute() and ".." not in path.parts)
    if name in (CLIENT, NFT):
        return "program"
    if name.startswith("wheels/") and len(path.parts) == 2 and path.name in WHEELS:
        return "wheel"
    if path.is_relative_to(LIB):
        parts = path.relative_to(LIB).parts
        if (0 < len(parts) <= 5 and all(re.fullmatch(r"[A-Za-z0-9+_.-]+", p) for p in parts)
                and re.fullmatch(r"[A-Za-z0-9+_.-]+\.so(?:\.[A-Za-z0-9_.-]+)?", path.name)):
            return "library"
    return None


def trusted_parent(parent):
    # No named redirect or writable ancestor is permitted in the fresh VM.
    for directory in (*reversed(parent.parents), parent):
        if not directory.exists():
            directory.mkdir(mode=0o755)
        info = directory.lstat()
        need(stat.S_ISDIR(info.st_mode) and (info.st_uid, info.st_gid) == (0, 0) and not info.st_mode & 0o022)


def sink(path, source, mode, size, expected=None):
    trusted_parent(path.parent)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, mode)
    record = hashlib.sha256()
    count = 0
    with os.fdopen(fd, "wb") as target:
        os.fchmod(target.fileno(), mode)
        while block := source.read(131072):
            count += len(block)
            need(count <= size)
            record.update(block)
            target.write(block)
    need(count == size and (expected is None or (record.hexdigest() == expected and digest_path(path, size) == expected)))


def copy(source, target, mode, bound, expected=None):
    fd = os.open(source, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
    with os.fdopen(fd, "rb") as original:
        item = os.fstat(original.fileno())
        need(stat.S_ISREG(item.st_mode) and item.st_nlink == 1 and 0 < item.st_size <= bound)
        sink(target, original, mode, item.st_size, expected)
        need(file_stamp(os.fstat(original.fileno())) == file_stamp(source.lstat()) == file_stamp(item))


def place_runtime(source, approved):
    with tarfile.open(source, "r:") as bundle:
        members = {}
        total = 0
        for item in bundle:
            role = member_kind(item.name)
            need(role is not None and item.name not in members and len(members) < MEMBER_LIMIT
                 and (item.isfile() or role == "library" and item.issym())
                 and 0 <= item.size <= 32 * 1024 * 1024)
            total += item.size
            need(total <= PAYLOAD_MAX)
            members[item.name] = item, role
        need({CLIENT, NFT, *("wheels/" + name for name in WHEELS)} <= set(members))
        for name, (item, role) in members.items():
            if item.issym():
                relative = PurePosixPath(item.linkname)
                target = str(PurePosixPath(name).parent / relative)
                need(len(relative.parts) == 1 and relative.name not in (".", "..") and target != name
                     and target in members and members[target][0].isfile())
        for name, (item, role) in members.items():
            target = TEMP / PurePosixPath(name).name if role == "wheel" else Path("/") / name
            if item.issym():
                trusted_parent(target.parent)
                target.symlink_to(item.linkname)
                continue
            needed = approved["client_elf_sha256"] if name == CLIENT else None
            if role == "wheel":
                count, needed = WHEELS[target.name]
                need(item.size == count)
            with bundle.extractfile(item) as data:
                sink(target, data, 0o755 if role == "program" else 0o644, item.size, needed)


@dataclass(frozen=True)
class Inputs:
    case: str
    providers: dict
    client: Path
    wheels: tuple
    ubuntu_smbclient: dict


def install_inputs(seed, request):
    approved = manifest(seed, request)
    package_table(seed)
    pair = json.loads(read_regular(seed / "pair.json"), object_pairs_hook=object_once)
    offline = json.loads(read_regular(seed / "offline.json"), object_pairs_hook=object_once)
    shared = ("source", "challenge", "guest_inputs_sha256")
    need(pair["profile"] == "SMB-MOVE-PAIR-1" and offline["profile"] == "SMB-MOVE-OFFLINE-1"
         and all(pair[k] == offline[k] == request[k] for k in shared)
         and all(pair[what + "_sha256"] == request[what + "_sha256"] for what in ("server", "helper"))
         and all(hex_id(offline[data + "_sha256"], 64) for data in ("runtime", "history")))
    need(digest_path(seed / "runtime", PAYLOAD_MAX) == offline["runtime_sha256"]
         and digest_path(seed / "history", ELF_MAX) == offline["history_sha256"]
         and digest_path(seed / "zxs", ELF_MAX) == request["server_sha256"]
         and digest_path(seed / "helper", ELF_MAX) == request["helper_sha256"])
    # Only existing canonical distro members from the authenticated root disk.
    providers = {}
    original = {}
    for role, directory_name in PROVIDER_NAMES.items():
        row = approved["providers"][role]
        base = Path("/") / LIB
        soname, real = base / directory_name, base / row["member"]
        need(soname.is_symlink() and soname.readlink() == Path(row["member"])
             and real.is_file() and real.lstat().st_uid == 0 and real.lstat().st_gid == 0
             and digest_path(real, ELF_MAX) == row["sha256"])
        providers[role] = row["sha256"]
        original[role] = real
    need(not os.path.lexists("/zxs") and not os.path.lexists(PROVIDER_DIR.parent)
         and not os.path.lexists(TEMP) and not os.path.lexists("/" + CLIENT) and not os.path.lexists("/" + NFT))
    trusted_parent(Path("/zxs/state"))
    trusted_parent(Path("/zxs/bin"))
    trusted_parent(TEMP)
    for src, dest, purpose in (("zxs", "zxs", "server"), ("helper", "helper", "helper")):
        copy(seed / src, Path("/zxs/bin") / dest, 0o755, ELF_MAX, request[purpose + "_sha256"])
    copy(seed / "history", Path("/zxs/bin/move-history-test"), 0o755, ELF_MAX, offline["history_sha256"])
    client = TEMP / "client.py"
    copy(seed / "rust_smb_client.py", client, 0o644, 256 * 1024)
    for role, real in original.items():
        copy(real, PROVIDER_DIR / PROVIDER_NAMES[role], 0o644, ELF_MAX, providers[role])
    place_runtime(seed / "runtime", approved)
    return Inputs(request["case"], providers, client, tuple(TEMP / name for name in WHEELS),
                  {"package_sha256": CLIENT_PACKAGE, "elf_sha256": approved["client_elf_sha256"], "version": CLIENT_BANNER})
