"""Qualify and use only this disposable guest's four images and one local link."""

from contextlib import ExitStack, contextmanager
from dataclasses import dataclass
import fcntl
import json
import os
from pathlib import Path
import re
import select
import signal
import stat
import subprocess
import time
from struct import unpack

from guest_smb_move_foreground import ENV, GID, UID, require
from guest_smb_move_media import (DISK_BY_ID, GUEST_PARENT, PART_BYTES, ROLES, _mount_index,
                                  device, mountpoint, observe_roles, selected, serial)

IP = "/usr/sbin/ip"
NSENTER = "/usr/bin/nsenter"
SETPRIV = "/usr/bin/setpriv"
HOST_LINK = "movehost0"
CLIENT_LINK = "zxsaccept0"
MAX_COMMAND_OUTPUT = 64 * 1024


def run(argv, *, fds=(), request=None, capture=False, timeout=25):
    result = subprocess.run(argv, env=ENV, cwd="/", input=request,
                            stdin=subprocess.DEVNULL if request is None else None,
                            stdout=subprocess.PIPE if capture else subprocess.DEVNULL,
                            stderr=subprocess.DEVNULL, timeout=timeout, check=True, pass_fds=fds)
    if capture:
        require(len(result.stdout) <= MAX_COMMAND_OUTPUT)
        return result.stdout
    return None


def coordinate(device_number):
    return f"{os.major(device_number)}:{os.minor(device_number)}"


def media_devices(stack):
    mounted = {row[2] for row in _mount_index()}
    seen, answer = set(), {}
    for role, name in ROLES.items():
        link = device(name)
        dest = link.resolve(strict=True)
        require(link.parent == DISK_BY_ID and link.is_symlink() and dest.parent == Path("/dev")
                and re.fullmatch(r"vd[a-z]+", dest.name))
        fd = os.open(dest, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
        stack.callback(os.close, fd)
        info = os.fstat(fd)
        ident = coordinate(info.st_rdev)
        sysfs = Path("/sys/dev/block") / ident
        require(stat.S_ISBLK(info.st_mode) and info.st_rdev != 0 and info.st_rdev not in seen
                and ident.encode("ascii") not in mounted and (sysfs / "dev").read_text().strip() == ident
                and not (sysfs / "partition").exists() and (sysfs / "serial").read_text().strip() == serial(name)
                and unpack("=Q", fcntl.ioctl(fd, 0x80081272, bytes(8)))[0] == PART_BYTES
                and os.path.samestat(info, link.stat()))
        seen.add(info.st_rdev)
        answer[role] = (name, dest, info)
    return answer


def mount_fixture():
    # Format was applied only to the four new private host image files.
    # Here re-qualify every virtio disk before any guest mount or directory write.
    with ExitStack() as held:
        disks = media_devices(held)
        GUEST_PARENT.mkdir(mode=0o755)
        for name, path, original in disks.values():
            require(os.path.samestat(original, path.stat(follow_symlinks=False))
                    and os.path.samestat(original, device(name).stat()))
            mountpoint(name).mkdir(mode=0o755)
            run(("/usr/bin/mount", "-t", "ext4", "-o", "rw,nosuid,nodev,noexec",
                 str(path), str(mountpoint(name))))
            leaf = selected(name)
            leaf.mkdir(mode=0o700)
            os.chown(leaf, UID, GID, follow_symlinks=False)
        return observe_roles(UID, GID, empty=True)


def namespace_id(meta):
    return [meta.st_dev, meta.st_ino]


@dataclass(frozen=True)
class Client:
    fd: int
    identity: list
    outside: list

    def command(self, argv, *, fds=(), request=None, capture=False, timeout=25):
        return run((NSENTER, f"--net=/proc/self/fd/{self.fd}", "--", *argv),
                   fds=(self.fd, *fds), request=request, capture=capture, timeout=timeout)

    def unprivileged(self, argv, *, fds):
        return self.command((SETPRIV, "--reuid", str(UID), "--regid", str(GID), "--clear-groups",
                             "--no-new-privs", "--", *argv), fds=fds, capture=True, timeout=80)


def ipv6_off(name):
    control = Path("/proc/sys/net/ipv6/conf") / name / "disable_ipv6"
    with control.open("w", encoding="ascii") as f:
        f.write("1\n")
    require(control.read_bytes() == b"1\n")


@contextmanager
def peer_namespace():
    opened, done = os.pipe2(os.O_CLOEXEC)
    read_ack, write_ack = os.pipe2(os.O_CLOEXEC)
    child = os.fork()
    if child == 0:
        os.close(done)
        os.close(read_ack)
        try:
            os.unshare(os.CLONE_NEWNET)
            if os.write(write_ack, b"N") != 1:
                os._exit(125)
            os.close(write_ack)
            os._exit(0 if os.read(opened, 1) == b"" else 125)
        except OSError:
            os._exit(125)
    os.close(opened)
    os.close(write_ack)
    with ExitStack() as held:
        held.callback(os.close, read_ack)
        fd = os.pidfd_open(child)
        held.callback(os.close, fd)
        try:
            require(select.select((read_ack,), (), (), 6)[0] and os.read(read_ack, 1) == b"N")
            peer = os.open(f"/proc/{child}/ns/net", os.O_RDONLY | os.O_CLOEXEC)
            held.callback(os.close, peer)
            identity = namespace_id(os.fstat(peer))
            outside = namespace_id(os.stat("/proc/thread-self/ns/net"))
            require(identity != outside and not select.select((fd,), (), (), 0)[0])
            yield Client(peer, identity, outside)
        finally:
            os.close(done)
            if not select.select((fd,), (), (), 8)[0]:
                signal.pidfd_send_signal(fd, signal.SIGTERM)
                require(select.select((fd,), (), (), 8)[0])
            actual, status = os.waitpid(child, 0)
            require(actual == child and status == 0)


FILTER = b"""table inet smb_move_fixture {
  chain incoming {
    type filter hook input priority 0; policy drop;
    iifname "zxsaccept0" ip saddr 192.0.2.1 ip daddr 192.0.2.2 tcp sport 445 accept
  }
  chain outgoing {
    type filter hook output priority 0; policy drop;
    oifname "zxsaccept0" ip saddr 192.0.2.2 ip daddr 192.0.2.1 tcp dport 445 accept
  }
}
"""


@contextmanager
def isolated_link():
    require(os.listdir("/sys/class/net") == ["lo"])
    try:
        with peer_namespace() as peer:
            # Only the two fixed new guest interfaces. There is no virtual
            # hardware NIC, bridge, default route, forwarding or external DNS.
            run((IP, "link", "add", HOST_LINK, "type", "veth", "peer", "name", CLIENT_LINK))
            ipv6_off(HOST_LINK)
            # iproute2 6.19 passes this retained original namespace as IFLA_NET_NS_FD.
            run((IP, "link", "set", "dev", CLIENT_LINK, "netns", f"/proc/self/fd/{peer.fd}"), fds=(peer.fd,))
            peer.command(("/usr/bin/python3", "-I", "-c",
                          "from pathlib import Path; p=Path('/proc/sys/net/ipv6/conf/zxsaccept0/disable_ipv6'); "
                          "p.write_text('1\\n'); assert p.read_bytes() == b'1\\n'"))
            peer.command(("/usr/sbin/nft", "-f", "-"), request=FILTER)
            # The fixed deny-by-default two-chain ruleset must actually exist;
            # failure of the approved kernel modules is not a skip/fallback.
            listing = peer.command(("/usr/sbin/nft", "-j", "-n", "list", "ruleset"), capture=True)
            nft = json.loads(listing)["nftables"]
            def matches(left, right):
                return {"match": {"op": "==", "left": left, "right": right}}

            def ipv4(field, addr):
                return matches({"payload": {"protocol": "ip", "field": field}}, addr)

            bases = [item["chain"] for item in nft if "chain" in item]
            rules = {row["rule"]["chain"]: row["rule"]["expr"] for row in nft if "rule" in row}
            require(len(nft) == 6 and len(bases) == 2 and len(rules) == 2
                    and {(row["name"], row["hook"], row["policy"]) for row in bases}
                    == {("incoming", "input", "drop"), ("outgoing", "output", "drop")}
                    and rules["incoming"] == [matches({"meta": {"key": "iifname"}}, CLIENT_LINK),
                                              ipv4("saddr", "192.0.2.1"), ipv4("daddr", "192.0.2.2"),
                                              matches({"payload": {"protocol": "tcp", "field": "sport"}}, 445), {"accept": None}]
                    and rules["outgoing"] == [matches({"meta": {"key": "oifname"}}, CLIENT_LINK),
                                              ipv4("saddr", "192.0.2.2"), ipv4("daddr", "192.0.2.1"),
                                              matches({"payload": {"protocol": "tcp", "field": "dport"}}, 445), {"accept": None}])
            require(sorted(os.listdir("/sys/class/net")) == ["lo", HOST_LINK]
                    and namespace_id(os.stat("/proc/thread-self/ns/net")) == peer.outside)
            run((IP, "addr", "add", "192.0.2.1/30", "dev", HOST_LINK))
            run((IP, "link", "set", "dev", HOST_LINK, "up"))
            peer.command((IP, "addr", "add", "192.0.2.2/30", "dev", CLIENT_LINK))
            peer.command((IP, "link", "set", "dev", CLIENT_LINK, "up"))
            peer.command((IP, "link", "set", "dev", "lo", "up"))
            yield peer
    finally:
        limit = time.monotonic() + 2
        while os.listdir("/sys/class/net") != ["lo"] and time.monotonic() < limit:
            time.sleep(0.03)
        require(os.listdir("/sys/class/net") == ["lo"])
