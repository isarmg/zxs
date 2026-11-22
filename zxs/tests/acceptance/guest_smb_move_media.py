"""Four disposable whole ext4 images, and the exact guest directory witness."""

from pathlib import Path
import fcntl
import os
import stat
from uuid import UUID

ROLES = {
    "S": "system",
    "F1": "source",
    "F2": "target",
    "R": "recycle",
}
PART_BYTES = 256 * 1024 * 1024
GUEST_PARENT = Path("/mnt/smb-move")
DISK_BY_ID = Path("/dev/disk/by-id")
READ_DIR = os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC | os.O_NOFOLLOW
FS_IOC_GETFSUUID = 0x80111500  # Linux UAPI; struct fsuuid2 is one length and 16 bytes.
MAX_MOUNTINFO = 4 * 1024 * 1024


def image_name(name):
    return name + ".ext4"


def serial(name):
    return "smb-move-" + name


def device(name):
    return DISK_BY_ID / ("virtio-" + serial(name))


def mountpoint(name):
    return GUEST_PARENT / name


def selected(name):
    # Keep lost+found outside the admitted originally empty directory.
    return mountpoint(name) / "fixture"


def fresh_images(case, mkfs):
    """Stage only new private files; mkfs is the exact previously verified tool."""
    for name in ROLES.values():
        image = case / image_name(name)
        with image.open("xb") as original:
            os.ftruncate(original.fileno(), PART_BYTES)
        mkfs("-q", "-t", "ext4", "-L", "move-" + name, str(image))


def virtio_args():
    """For QEMU cwd set to the newly prepared private case directory."""
    result = []
    for name in ROLES.values():
        result.extend(("-drive", f"if=none,id={name},format=raw,file={image_name(name)}",
                       "-device", f"virtio-blk-pci,drive={name},serial={serial(name)}"))
    return result


def _mount_index():
    with open("/proc/thread-self/mountinfo", "rb") as source:
        raw = source.read(MAX_MOUNTINFO + 1)
    if not raw.endswith(b"\n") or len(raw) > MAX_MOUNTINFO:
        raise RuntimeError("cannot bound guest mountinfo")
    return [row.split() for row in raw.splitlines()]


def _mnt_id(fd):
    with open(f"/proc/thread-self/fdinfo/{fd}", "rb") as source:
        found = [row.split()[1] for row in source if row.startswith(b"mnt_id:")]
    if len(found) != 1 or not found[0].isdigit() or int(found[0]) <= 0:
        raise RuntimeError("cannot prove exact mounted descriptor")
    return int(found[0])


def _observe_role(name, parent, mount_rows, uid, gid, empty):
    expected = os.stat(device(name))
    if not stat.S_ISBLK(expected.st_mode) or expected.st_rdev == 0:
        raise RuntimeError("guest role device is not its original virtual block device")
    mount = os.open(name, READ_DIR, dir_fd=parent)
    try:
        held = os.fstat(mount)
        mid = _mnt_id(mount)
        coord = f"{os.major(held.st_dev)}:{os.minor(held.st_dev)}".encode("ascii")
        role_path = os.fsencode(mountpoint(name))
        matching = [r for r in mount_rows if len(r) > 9 and r[4] == role_path]
        if len(matching) != 1:
            raise RuntimeError("guest role is not a uniquely mounted whole filesystem")
        row = matching[0]
        sep = row.index(b"-")
        if (row[0] != str(mid).encode("ascii") or row[2] != coord or row[3] != b"/"
                or held.st_dev != expected.st_rdev or held.st_ino == 0
                or row[sep + 1] != b"ext4" or b"rw" not in row[sep + 3].split(b",")
                or not {b"rw", b"nosuid", b"nodev", b"noexec"}.issubset(row[5].split(b","))):
            raise RuntimeError("guest role is not on the expected whole writable ext4 root")

        raw = bytearray(17)
        fcntl.ioctl(mount, FS_IOC_GETFSUUID, raw)
        if raw[0] != 16 or not any(raw[1:]):
            raise RuntimeError("cannot verify original ext4 UUID")
        fsuuid = UUID(bytes=bytes(raw[1:])).hex
        leaf = os.open("fixture", READ_DIR, dir_fd=mount)
        try:
            meta = os.fstat(leaf)
            if (meta.st_dev != held.st_dev or meta.st_ino == 0 or meta.st_ino == held.st_ino
                    or _mnt_id(leaf) != mid or stat.S_IMODE(meta.st_mode) != 0o700
                    or (meta.st_uid, meta.st_gid) != (uid, gid)
                    or not os.path.samestat(meta, os.stat("fixture", dir_fd=mount, follow_symlinks=False))
                    or (empty and os.listdir(leaf))):
                raise RuntimeError("guest role leaf is not its original private workspace")
            if not os.path.samestat(held, os.stat(name, dir_fd=parent, follow_symlinks=False)):
                raise RuntimeError("guest role mountpoint changed during inspection")
            return held.st_dev, mid, fsuuid, meta.st_ino
        finally:
            os.close(leaf)
    finally:
        os.close(mount)


def observe_roles(uid, gid, *, empty):
    """Observe only the four exact role paths; compare the same result after the case."""
    if not 0 < uid < 2**32 - 1 or not 0 < gid < 2**32 - 1:
        raise ValueError("need the original non-root storage identity")
    parent = os.open(GUEST_PARENT, READ_DIR)
    try:
        mount_rows = _mount_index()
        observed = {role: _observe_role(name, parent, mount_rows, uid, gid, empty)
                    for role, name in ROLES.items()}
        if not all(len({item[column] for item in observed.values()}) == len(ROLES)
                   for column in (0, 1, 2)):
            raise RuntimeError("guest roles did not get four distinct whole block filesystems")
        return observed
    finally:
        os.close(parent)
