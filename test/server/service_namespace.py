#!/usr/bin/env python3
"""Run one daemon or helper integration test with a private administrative host root."""
from pathlib import Path
import os
import subprocess
import sys
import tempfile

CHILD = "ZXS_SERVICE_STARTUP_MOUNT_NAMESPACE"


def mount(*arguments):
    subprocess.run(["/usr/bin/mount", *map(str, arguments)], check=True)


def bind(source, destination, writable=False):
    mount("--bind", source, destination)
    if not writable:
        mount("--options", "remount,bind,ro", destination)


def prepare_root(root, parent_namespace):
    if os.geteuid() != 0:
        raise RuntimeError("the namespace fixture requires its elevated child")
    if os.readlink("/proc/self/ns/mnt") == parent_namespace:
        raise RuntimeError("the fixture requires a separate mount namespace")
    mount("--types", "tmpfs", "--options", "mode=755", "tmpfs", root)
    # Nonrecursive binds keep the staging mount out of its own /tmp view.
    # The test owns /tmp files; other host paths are read-only in this namespace.
    with os.scandir("/") as entries:
        for entry in entries:
            if entry.name == "zxs":
                continue
            destination = root / entry.name
            source = Path("/") / entry.name
            if entry.is_symlink():
                destination.symlink_to(os.readlink(source))
            elif entry.is_dir():
                destination.mkdir()
                bind(source, destination, writable=entry.name == "tmp")
            elif entry.is_file():
                destination.touch()
                bind(source, destination)
    (root / "zxs").mkdir(mode=0o755)
    (root / "zxs").chmod(0o755)


def child_command(root, namespace, executable, name):
    return [
        "/usr/bin/sudo", "-n", "--", "/usr/bin/env", "-i",
        "PATH=/usr/bin:/usr/sbin:/bin:/sbin", "TMPDIR=/tmp",
        "/usr/bin/timeout", "--signal=TERM", "--kill-after=5s", "240s",
        "/usr/bin/unshare", "--mount", "--propagation", "private", "--",
        "/usr/bin/python3", str(Path(__file__).resolve()), "--inside",
        str(root), namespace, executable, name,
    ]


def main(arguments):
    if arguments[0] == "--inside":
        _, root, parent_namespace, executable, name = arguments
        root = Path(root)
        prepare_root(root, parent_namespace)
        os.chroot(root)
        os.chdir("/")
        os.environ[CHILD] = name
        os.execv(executable, [executable, "--exact", name, "--nocapture", "--test-threads=1"])
    executable, name = arguments
    namespace = os.readlink("/proc/self/ns/mnt")
    with tempfile.TemporaryDirectory(prefix="zxs-service-startup-") as temporary:
        status = subprocess.run(child_command(Path(temporary), namespace, executable, name))
        return status.returncode


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
