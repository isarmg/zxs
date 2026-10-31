"""Small complete release fixture; no compiler or executable product required."""
import hashlib
import json
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
    runtime = {"schema_version": 1, "distribution": "debian", "release": "13", "architecture": "amd64",
               "binaries": [{"name": name, "sha256": fields[f"native-{name}-sha256"], "needed": ["libc.so.6"],
                             "libraries": [{"soname": "libc.so.6", "path": "/lib/libc.so.6",
                                            "resolved_path": "/usr/lib/libc.so.6", "sha256": "a" * 64,
                                            "package": "libc6:amd64", "version": "fixture-only"}]}
                            for name in ("zxs", "zxs-rescue")]}
    text = json.dumps(runtime, separators=(",", ":"), sort_keys=True)
    fields["native-runtime-dependencies"] = text
    fields["native-runtime-dependencies-sha256"] = hashlib.sha256(text.encode()).hexdigest()


def prepare(root):
    root = Path(root)
    spec = json.loads((ROOT / SPEC).read_text())
    for directory in spec["directories"]:
        (root / directory).mkdir(parents=True, exist_ok=True)
    for relative in [*spec["files"], "README.md", "compliance/NOTICE.md", "docs/README.md", "deploy/systemd/zxs.service",
                     "deploy/samba/smb.conf.example",
                     "target/native/x86_64-unknown-linux-gnu/release/zxs", "target/native/x86_64-unknown-linux-gnu/release/zxs-rescue", "build/web/index.html", "build/web/boot.js",
                     "build/web/zxs.css", "build/web/styles/startup.css", "build/web/pkg/zxs.js",
                     "build/web/pkg/zxs_bg.wasm", "build/web/pkg/zxs_bg.wasm.gz"]:
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        if not path.exists():
            path.write_text(f"fixture {relative}\n")
    for name in ("zxs", "zxs-rescue"):
        (root / "target/native/x86_64-unknown-linux-gnu/release" / name).chmod(0o755)
    native = root / "target/native/x86_64-unknown-linux-gnu/release"
    (native / "zxs").write_bytes(elf_fixture())
    fields = {"version": "0.1.0", "commit": "0123456789012345678901234567890123456789",
              "native-target": "x86_64-unknown-linux-gnu", "native-profile": "release",
              "native-rustc": "fixture compiler", "native-environment-sha256": "a" * 64,
              "native-inputs-sha256": "b" * 64}
    for name in ("zxs", "zxs-rescue"):
        fields[f"native-{name}-sha256"] = hashlib.sha256((native / name).read_bytes()).hexdigest()
    add_runtime_fields(fields)
    (native / "BUILD.txt").write_text("".join(f"{key}={value}\n" for key, value in fields.items()))
    for relative in (SPEC, ".github/scripts/release-package.sh", ".github/scripts/verify-web-dist.py",
                     ".github/scripts/verify-release-assets.py"):
        shutil.copy2(ROOT / relative, root / relative)
    manifest = record(root)
    fields.update({"native-baseline": "debian13-amd64", "native-web-embedded": "true",
                   "native-web-commit": fields["commit"], "native-web-tree": "b" * 40,
                   "native-web-manifest-sha256": hashlib.sha256((root / "build/web/build-manifest.json").read_bytes()).hexdigest()})
    add_runtime_fields(fields)
    (native / "BUILD.txt").write_text("".join(f"{key}={value}\n" for key, value in fields.items()))
    vfs = root / "target/smb-debian13/product"
    vfs.mkdir(parents=True, exist_ok=True)
    (vfs / "zxs.so").write_bytes(elf_fixture(module=True))
    (vfs / "VFS-BUILD.json").write_text(json.dumps(vfs_receipt(fields["commit"], fields["native-web-tree"], (vfs / "zxs.so").read_bytes())))
    return manifest


def vfs_receipt(commit, tree, module):
    return {"distribution": "debian", "release": "13", "architecture": "amd64",
            "package_version": "2:4.22.11+dfsg-0+deb13u1", "vfs_interface": 50,
            "commit": commit, "tree": tree, "module_sha256": hashlib.sha256(module).hexdigest()}


def write_release_assets(directory, commit, version="0.1.0"):
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    data = b"release asset bytes"
    for name in ("INSTALL.md", f"zxs-{version}-debian13-amd64", f"zxs-vfs-{version}-debian13-amd64.so", f"zxs-materials-{version}.tar.gz"):
        (directory / name).write_bytes(data)
    (directory / f"zxs-{version}-debian13-amd64").write_bytes(elf_fixture())
    (directory / f"zxs-vfs-{version}-debian13-amd64.so").write_bytes(elf_fixture(module=True))
    fields = {"version": version, "commit": commit, "native-target": "x86_64-unknown-linux-gnu",
              "native-profile": "release", "native-rustc": "fixture compiler",
              "native-environment-sha256": "a" * 64, "native-inputs-sha256": "b" * 64,
              "native-zxs-sha256": hashlib.sha256(elf_fixture()).hexdigest(), "native-zxs-rescue-sha256": "d" * 64,
              "materials-archive-sha256": hashlib.sha256(data).hexdigest(),
              "native-baseline": "debian13-amd64", "native-web-embedded": "true",
              "native-web-commit": commit, "native-web-tree": "b" * 40,
              "native-web-manifest-sha256": "c" * 64}
    add_runtime_fields(fields)
    (directory / "BUILD.txt").write_text("".join(f"{key}={value}\n" for key, value in fields.items()))
    (directory / "VFS-BUILD.json").write_text(json.dumps(vfs_receipt(commit, "b" * 40, elf_fixture(module=True))))
    return fields


if __name__ == "__main__":
    prepare(Path(sys.argv[1]))
