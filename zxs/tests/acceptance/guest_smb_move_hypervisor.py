"""The staged, signed TCG inputs for the isolated MOVE job."""

from hashlib import sha256
import os
from pathlib import Path
import re
import subprocess

# Reuse the exact guest image and Ubuntu tool snapshot used by the disposable
# guest fixtures. The host package manager always has private state.
IMAGE = "ubuntu-26.04-minimal-cloudimg-amd64.img"
IMAGE_RELEASE = "https://cloud-images.ubuntu.com/minimal/releases/resolute/release-20261118"
IMAGE_SHA = "f6c630c04d4fad012f2c4ed3a21a2b0c0f814f13f6873568da4c31d8df65c12e"
CLOUD_KEY = "/usr/share/keyrings/ubuntu-cloudimage-keyring.gpg"
CLOUD_SIGNER = "D2EB44626FDDC30B513D5BB71A5D6C4C7DB87C81"
SNAPSHOT_COORDINATE = "20261118T120000Z"
SNAPSHOT = "https://snapshot.ubuntu.com/ubuntu/" + SNAPSHOT_COORDINATE
ARCHIVE_KEY = "/etc/apt/trusted.gpg.d/ubuntu-keyring-2018-archive.gpg"
INDEX = {
    "noble": "cdb2f31d809f589719a53c6ad15f255b27569c4059542ada282aaa21b8e164b0",
    "noble-updates": "a4dfe9c58d77db89e8b45240d1cc14a3e6dd9159bc29395aebfe6e893594dccb",
    "noble-security": "65a6f9c5cf5b8d887e45c2a638b19bedd86e9d3630afee8b3a7b46ea5803dc22",
}
PACKAGES = ("qemu-system-x86", "ovmf", "genisoimage", "e2fsprogs")
TOOLS = {
    "iso": "usr/bin/genisoimage",
    "mkfs": "usr/sbin/mke2fs",
    "qemu": "usr/bin/qemu-system-x86_64",
}
ARCH_LIB = "usr/lib/x86_64-linux-gnu"


def file_sha(path):
    with path.open("rb") as stream:
        h = sha256()
        for data in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(data)
    return h.hexdigest()


def command(args, area, *, extra=None, output=False, timeout=240):
    env = {"PATH": "/usr/bin:/bin", "LC_ALL": "C", "TZ": "UTC",
           "HOME": str(area), "TMPDIR": str(area)}
    if extra is not None:
        env.update(extra)
    return subprocess.run(args, check=True, cwd=area, env=env, stdin=subprocess.DEVNULL,
                          stdout=subprocess.PIPE if output else subprocess.DEVNULL,
                          stderr=subprocess.DEVNULL, timeout=timeout).stdout


def download(area, asset, limit):
    command(("/usr/bin/curl", "--fail", "--silent", "--show-error", "--location",
             "--proto", "=https", "--proto-redir", "=https", "--connect-timeout", "15",
             "--max-time", "210", "--retry", "2", "--max-filesize", str(limit),
             f"{IMAGE_RELEASE}/{asset}", "--output", asset), area)


def verified_image(area):
    for asset in ("SHA256SUMS", "SHA256SUMS.gpg"):
        download(area, asset, 4 * 1024 * 1024)
    gnupg = area / "gnupg"
    gnupg.mkdir(mode=0o700)
    signatures = command(("/usr/bin/gpgv", "--keyring", CLOUD_KEY, "--status-fd", "1",
                          "SHA256SUMS.gpg", "SHA256SUMS"), area,
                         extra={"GNUPGHOME": str(gnupg)}, output=True)
    if not any(row.startswith(f"[GNUPG:] VALIDSIG {CLOUD_SIGNER} ".encode("ascii"))
               for row in signatures.splitlines()):
        raise RuntimeError("guest image index lacks the approved signer")
    if (area / "SHA256SUMS").read_bytes().splitlines().count(f"{IMAGE_SHA} *{IMAGE}".encode("ascii")) != 1:
        raise RuntimeError("guest image index changed approved artifact")
    download(area, IMAGE, 600_000_000)
    image = area / IMAGE
    if file_sha(image) != IMAGE_SHA:
        raise RuntimeError("guest image digest mismatch")
    return image


def verified_tools(area):
    apt = area / "apt"
    tools = area / "tools"
    for leaf in ("etc/empty", "lists/partial", "archives/partial"):
        (apt / leaf).mkdir(mode=0o700, parents=True)
    tools.mkdir(mode=0o700)
    (apt / "status").touch(exist_ok=False)
    config = apt / "config"
    config.write_text(
        f'Dir::Etc "{apt}/etc";\n'
        'Dir::Etc::main "/dev/null";\nDir::Etc::parts "empty";\n'
        'Dir::Etc::sourceparts "empty";\nDir::Etc::sourcelist "sources.list";\n'
        f'Dir::State "{apt}";\nDir::State::status "{apt}/status";\n'
        f'Dir::Cache "{apt}";\nDir::Log "{apt}";\n'
        'Dir::Cache::pkgcache "";\nDir::Cache::srcpkgcache "";\n'
        'Acquire::Languages "none";\nAcquire::Retries "2";\n'
        'APT::Get::Always-Include-Phased-Updates "true";\nAPT::Update::Error-Mode "any";\n',
        encoding="ascii",
    )
    (apt / "etc/sources.list").write_text("".join(
        f"deb [arch=amd64 signed-by={ARCHIVE_KEY}] {SNAPSHOT} {suite} main universe\n"
        for suite in INDEX), encoding="ascii")
    apt_env = {"APT_CONFIG": str(config)}
    command(("/usr/bin/apt-get", "update"), area, extra=apt_env)
    for suite, approved in INDEX.items():
        name = f"snapshot.ubuntu.com_ubuntu_{SNAPSHOT_COORDINATE}_dists_{suite}_InRelease"
        if file_sha(apt / "lists" / name) != approved:
            raise RuntimeError("guest tool snapshot changed")
    command(("/usr/bin/apt-get", "--assume-yes", "--download-only", "--no-install-recommends",
             "install", *PACKAGES), area, extra=apt_env)
    packages = sorted((apt / "archives").glob("*.deb"))
    if not packages:
        raise RuntimeError("guest tool extraction must have authenticated packages")
    for package in packages:
        command(("/usr/bin/dpkg-deb", "--extract", str(package), str(tools)), area)
    return tools


class Hypervisor:
    def __init__(self, image, tools):
        self.image = image
        self.tools = tools
        self.loader = tools / ARCH_LIB / "ld-linux-x86-64.so.2"
        self.library = f"{tools / ARCH_LIB}:{tools / 'lib/x86_64-linux-gnu'}"
        self.modules = tools / ARCH_LIB / "qemu"
        self.firmware = tools / "usr/share/OVMF"

    def invocation(self, role, args):
        return (str(self.loader), "--inhibit-cache", "--library-path", self.library,
                str(self.tools / TOOLS[role]), *args)

    def run(self, case, role, *args, timeout=240):
        command(self.invocation(role, args), case, timeout=timeout,
                extra={"QEMU_MODULE_DIR": str(self.modules),
                       "MKE2FS_CONFIG": str(self.tools / "etc/mke2fs.conf")})

    def require_private_linker_closure(self, area):
        for literal in TOOLS.values():
            loaded = command((str(self.loader), "--inhibit-cache", "--library-path",
                              self.library, "--list", str(self.tools / literal)), area,
                             extra={"QEMU_MODULE_DIR": str(self.modules)}, output=True)
            for row in loaded.splitlines():
                name = row.split()
                if len(name) > 2 and name[1] == b"=>" and not name[2].startswith(os.fsencode(self.tools) + b"/"):
                    raise RuntimeError("guest tool loaded from outside the approved tool tree")


def prepare(work):
    if os.geteuid() == 0 or not re.fullmatch(r"/[a-zA-Z0-9_./-]+", str(work)):
        raise RuntimeError("guest tools require the unprivileged private runner workspace")
    area = work / "hypervisor"
    area.mkdir(mode=0o700)
    anchor = Hypervisor(verified_image(area), verified_tools(area))
    anchor.require_private_linker_closure(area)
    return anchor
