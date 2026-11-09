"""Small complete release fixture; no compiler or executable product required."""
import hashlib
import json
import gzip
import io
import os
import runpy
import subprocess
import tarfile
import tempfile
from pathlib import Path
import shutil
import sys

ROOT = Path(__file__).resolve().parents[2]
SPEC = "tools/web-build/build-inputs.json"


def record(root):
    spec = json.loads((root / SPEC).read_text())
    inputs = set(spec["files"])
    for directory in spec["directories"]:
        inputs.update(path.relative_to(root).as_posix() for path in (root / directory).rglob("*") if path.is_file())
    dist = root / "build/web"
    manifest = {
        "runtime": "web-interface", "target": "wasm32-unknown-unknown", "wasm_bindgen": "0.2.129",
        "inputs": {path: hashlib.sha256((root / path).read_bytes()).hexdigest() for path in sorted(inputs)},
        "files": {path.relative_to(dist).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
                  for path in sorted(dist.rglob("*")) if path.is_file() and path.name != "build-manifest.json"},
    }
    (dist / "build-manifest.json").write_text(json.dumps(manifest))
    return manifest


def elf_fixture(module=False):
    """Minimal ABI header for packaging tests only; intentionally NOT runnable."""
    import struct
    data = bytearray(64)
    data[:7] = b"\x7fELF\x02\x01\x01"
    struct.pack_into("<HHI", data, 16, 3 if module else 2, 62, 1)
    struct.pack_into("<H", data, 52, 64)
    return bytes(data)


def add_runtime_fields(fields):
    # Synthetic package identity for receipt plumbing only, not runtime evidence.
    fields.update({"native-helper-license-materials-embedded": "true",
                   "native-helper-license-materials-sha256": "e" * 64,
                   "native-helper-license-manifest-sha256": "f" * 64,
                   "native-helper-license-file-count": "5",
                   "native-helper-license-commit": fields["commit"],
                   "native-helper-license-tree": fields.get("native-web-tree", "b" * 40)})
    runtime = {"schema_version": 1, "distribution": "debian", "release": "13", "architecture": "amd64",
               "binaries": [{"name": name, "sha256": fields[f"native-{name}-sha256"], "needed": ["libc.so.6"],
                             "libraries": [{"soname": "libc.so.6", "path": "/lib/libc.so.6",
                                            "resolved_path": "/usr/lib/libc.so.6", "sha256": "a" * 64,
                                            "package": "libc6:amd64", "version": "fixture-only"}]}
                            for name in ("zxs", "zxs-helper")]}
    text = json.dumps(runtime, separators=(",", ":"), sort_keys=True)
    fields["native-runtime-dependencies"] = text
    fields["native-runtime-dependencies-sha256"] = hashlib.sha256(text.encode()).hexdigest()


def add_vfs_fields(fields, receipt, module):
    fields.update({"native-helper-embedded-vfs": "true",
                   "native-helper-vfs-payload-sha256": hashlib.sha256(module).hexdigest(),
                   "native-helper-vfs-build-sha256": hashlib.sha256(receipt).hexdigest()})


def prepare(root):
    root = Path(root)
    spec = json.loads((ROOT / SPEC).read_text())
    for directory in spec["directories"]:
        (root / directory).mkdir(parents=True, exist_ok=True)
    for relative in [*spec["files"], "README.md", "compliance/NOTICE.md", "docs/README.md", "deploy/systemd/zxs.service",
                     "deploy/samba/smb.conf.example",
                     "target/native/x86_64-unknown-linux-gnu/release/zxs", "target/native/x86_64-unknown-linux-gnu/release/zxs-helper", "build/web/index.html", "build/web/boot.js",
                     "build/web/zxs.css", "build/web/styles/startup.css", "build/web/pkg/zxs.js",
                     "build/web/pkg/zxs_bg.wasm", "build/web/pkg/zxs_bg.wasm.gz"]:
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        if not path.exists():
            path.write_text(f"fixture {relative}\n")
    for name in ("zxs", "zxs-helper"):
        (root / "target/native/x86_64-unknown-linux-gnu/release" / name).chmod(0o755)
    native = root / "target/native/x86_64-unknown-linux-gnu/release"
    for name in ("zxs", "zxs-helper"):
        (native / name).write_bytes(elf_fixture())
    fields = {"version": "0.1.0",
              "native-target": "x86_64-unknown-linux-gnu", "native-profile": "release",
              "native-rustc": "fixture compiler", "native-environment-sha256": "a" * 64,
              "native-inputs-sha256": "b" * 64}
    for name in ("zxs", "zxs-helper"):
        fields[f"native-{name}-sha256"] = hashlib.sha256((native / name).read_bytes()).hexdigest()
    fields.update({"native-helper-embedded-zxs": "true", "native-helper-payload-sha256": fields["native-zxs-sha256"]})
    for relative in (SPEC, ".github/scripts/release-package.sh", ".github/scripts/verify-web-dist.py",
                     ".github/scripts/verify-release-assets.py", ".github/scripts/release-source.py",
                     ".github/scripts/release-source-bundle.py", "docs/templates/release-install.md"):
        (root / relative).parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / relative, root / relative)
    (root / ".gitignore").write_text("/target/\n/build/\n/release-assets/\n")
    git_environment = dict(os.environ, GIT_AUTHOR_NAME="fixture", GIT_AUTHOR_EMAIL="fixture@example.invalid",
                           GIT_COMMITTER_NAME="fixture", GIT_COMMITTER_EMAIL="fixture@example.invalid",
                           GIT_AUTHOR_DATE="2000-01-01T00:00:00+00:00",
                           GIT_COMMITTER_DATE="2000-01-01T00:00:00+00:00")
    for arguments in (["init", "-q"], ["add", "--all"], ["commit", "-qm", "release source fixture"]):
        subprocess.run(["git", "-C", str(root), *arguments], env=git_environment, check=True)
    fields["commit"] = subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()
    tree = subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD^{tree}"], text=True).strip()
    manifest = record(root)
    fields.update({"native-baseline": "debian13-amd64", "native-web-embedded": "true",
                   "native-web-commit": fields["commit"], "native-web-tree": tree,
                   "native-web-manifest-sha256": hashlib.sha256((root / "build/web/build-manifest.json").read_bytes()).hexdigest()})
    fields.update({"native-helper-embedded-zxs": "true", "native-helper-payload-sha256": fields["native-zxs-sha256"]})
    add_runtime_fields(fields)
    (native / "BUILD.txt").write_text("".join(f"{key}={value}\n" for key, value in fields.items()))
    vfs = root / "target/smb-debian13/product"
    vfs.mkdir(parents=True, exist_ok=True)
    (vfs / "zxs.so").write_bytes(elf_fixture(module=True))
    (vfs / "VFS-BUILD.json").write_text(json.dumps(vfs_receipt(fields["commit"], fields["native-web-tree"], (vfs / "zxs.so").read_bytes())))
    add_vfs_fields(fields, (vfs / "VFS-BUILD.json").read_bytes(), (vfs / "zxs.so").read_bytes())
    (native / "BUILD.txt").write_text("".join(f"{key}={value}\n" for key, value in fields.items()))
    return manifest


def vfs_receipt(commit, tree, module):
    return {"distribution": "debian", "release": "13", "architecture": "amd64",
            "package_version": "2:4.22.11+dfsg-0+deb13u1", "vfs_interface": 50,
            "commit": commit, "tree": tree, "module_sha256": hashlib.sha256(module).hexdigest()}


def write_release_assets(directory, version="0.1.0"):
    """Real Git identities and canonical archives around explicitly synthetic ELFs."""
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="release-source-fixture-") as temporary:
        root = Path(temporary)
        prepare(root)
        native = root / "target/native/x86_64-unknown-linux-gnu/release"
        fields = dict(line.split("=", 1) for line in (native / "BUILD.txt").read_text().splitlines())
        fields["version"] = version
        build = "".join(f"{key}={value}\n" for key, value in fields.items()).encode()
        bundle = runpy.run_path(str(ROOT / ".github/scripts/release-source-bundle.py"))
        bundle["prepare"](root, directory / f"zxs-source-{version}.tar.gz", version, build,
                          (root / "target/smb-debian13/product/VFS-BUILD.json").read_bytes())
        for name in ("zxs", "zxs-helper"):
            shutil.copy2(native / name, directory / f"{name}-{version}-debian13-amd64")
    return fields


class BundleMember:
    """Mutate one nested metadata member without using production validation."""
    def __init__(self, directory, name, version="0.1.0"):
        self.archive = Path(directory) / f"zxs-source-{version}.tar.gz"
        self.name = name

    def entries(self):
        with tarfile.open(self.archive, "r:gz") as archive:
            return [(entry, archive.extractfile(entry).read()) for entry in archive]

    def read_bytes(self):
        return next(data for entry, data in self.entries() if entry.name == self.name)

    def read_text(self):
        return self.read_bytes().decode()

    def replace(self, data):
        entries = self.entries()
        with self.archive.open("wb") as output:
            with gzip.GzipFile(fileobj=output, mode="wb", filename="", mtime=0) as compressed:
                with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as archive:
                    for entry, original in entries:
                        if entry.name == self.name:
                            if data is None:
                                continue
                            original = data
                            entry.size = len(data)
                        archive.addfile(entry, io.BytesIO(original))

    def write_bytes(self, data):
        self.replace(data)

    def write_text(self, text):
        self.replace(text.encode())

    def unlink(self):
        self.replace(None)

    def exists(self):
        return any(entry.name == self.name for entry, _ in self.entries())


def bundle_member(directory, name, version="0.1.0"):
    return BundleMember(directory, name, version)


if __name__ == "__main__":
    prepare(Path(sys.argv[1]))

