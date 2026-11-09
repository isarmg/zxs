#!/usr/bin/env python3
"""Build the two native release products and record their build-step identity.

This receipt binds selected inputs and output bytes, not full reproducibility,
compiler trust, or the deployment machine's CPU capabilities.
"""
import hashlib
import json
import os
import platform
import re
import runpy
from pathlib import Path
import subprocess

TARGET = "x86_64-unknown-linux-gnu"
OUTPUT = Path("target/native") / TARGET / "release"
BINARIES = ("zxs", "zxs-helper")


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def require_clean_source():
    if subprocess.check_output(["git", "status", "--porcelain", "--untracked-files=normal"], text=True):
        raise RuntimeError("native release requires a clean source checkout")


def target_defaults(compiler):
    """Probe only this compiler's unmodified target defaults, never Cargo flags."""
    cpus = subprocess.check_output([compiler, "--print", "target-cpus", "--target", TARGET], text=True)
    defaults = [line.split()[0] for line in cpus.splitlines() if "default target CPU" in line]
    if len(defaults) != 1 or not re.fullmatch(r"[A-Za-z0-9_.+-]+", defaults[0]):
        raise RuntimeError("native compiler did not identify one safe default target CPU")
    raw = subprocess.check_output([compiler, "--print", "cfg", "--target", TARGET], text=True)
    cfg = []
    for line in raw.splitlines():
        if line.startswith("target_"):
            if not re.fullmatch(r'target_[a-z_]+="[A-Za-z0-9_.+-]*"', line):
                raise RuntimeError("native compiler returned an unsupported target cfg format")
            cfg.append(line)
    if not cfg or 'target_arch="x86_64"' not in cfg:
        raise RuntimeError("native compiler default target cfg is absent or inconsistent")
    return defaults[0], sorted(cfg)


def override_presence(variables):
    # Presence only: never publish arbitrary override names or values.
    return {
        "native-build-overrides-present": bool(variables),
        "native-rust-flag-overrides-present": any(
            key in {"RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS"}
            or (key.startswith("CARGO_TARGET_") and key.endswith("_RUSTFLAGS"))
            for key in variables),
        "native-c-flag-overrides-present": any(
            key in {"CFLAGS", "CXXFLAGS", "HOST_CFLAGS", "TARGET_CFLAGS", "HOST_CXXFLAGS", "TARGET_CXXFLAGS", "CRATE_CC_NO_DEFAULTS"}
            or key.startswith(("CFLAGS_", "CXXFLAGS_")) for key in variables),
    }


def require_debian13():
    release = platform.freedesktop_os_release()
    if (release.get("ID"), release.get("VERSION_ID"), platform.machine()) != ("debian", "13", "x86_64"):
        raise RuntimeError("final native product must be built in Debian 13 amd64")


def runtime_dependencies(root):
    inspector = runpy.run_path(str(root / ".github/scripts/native-runtime-receipt.py"))
    return {"schema_version": 1, "distribution": "debian", "release": "13", "architecture": "amd64",
            "binaries": [inspector["binary_receipt"](name, root / OUTPUT / name) for name in BINARIES]}


def legal_materials(root, version, commit, tree, payload_sha256):
    builder = runpy.run_path(str(root / ".github/scripts/helper-legal-materials.py"))
    return builder["prepare"](root, root / OUTPUT, version, commit, tree, payload_sha256)


def embedded_vfs(root, commit, tree):
    """Snapshot the already-produced same-source SDK module, without executing it."""
    validator = runpy.run_path(str(root / ".github/scripts/verify-release-assets.py"))
    source = os.open(root / "target/smb-debian13/product",
                     os.O_RDONLY | os.O_NOFOLLOW | os.O_DIRECTORY)
    try:
        receipt_bytes = validator["read_metadata"](source, "VFS-BUILD.json")
        receipt = validator["vfs_identity"](receipt_bytes,
                                           {"commit": commit, "native-web-tree": tree})
        target = json.loads((root / ".github/samba-debian-target.json").read_bytes())
        keys = set(target) | {"commit", "tree", "module_sha256", "sdk_recipe_sha256",
                              "sdk_config_sha256", "validation_mode", "runtime_acceptance", "host_version"}
        if set(receipt) != keys or any(receipt.get(key) != value for key, value in target.items()):
            raise RuntimeError("VFS receipt differs from the exact Debian SDK source target")
        recipe_inputs = "".join(f"{digest(root / path)}  {path}\n" for path in
                                (".github/scripts/smb-sdk.sh", ".github/samba-debian-target.json"))
        host_version = f"Version {target['upstream_version']}-Debian-{target['package_version'].split(':', 1)[1]}"
        if (receipt.get("sdk_recipe_sha256") != hashlib.sha256(recipe_inputs.encode()).hexdigest()
                or not re.fullmatch(r"[0-9a-f]{64}", receipt.get("sdk_config_sha256", ""))
                or receipt.get("host_version") != host_version
                or receipt.get("validation_mode") not in ("full", "build-only")
                or receipt.get("runtime_acceptance") != "not-run-by-build-script"):
            raise RuntimeError("VFS receipt does not bind the current SDK recipe and host")
        with validator["open_regular"](source, "zxs.so") as module:
            if os.fstat(module.fileno()).st_size > 64 * 1024 * 1024:
                raise RuntimeError("VFS module exceeds its embedded size bound")
            data = module.read(64 * 1024 * 1024 + 1)
        if len(data) > 64 * 1024 * 1024:
            raise RuntimeError("VFS module grew beyond its embedded size bound")
        validator["elf_identity"](data[:64], module=True)
        checksum = hashlib.sha256(data).hexdigest()
        if checksum != receipt["module_sha256"]:
            raise RuntimeError("VFS product differs from the matching SDK receipt")
    finally:
        os.close(source)
    output = root / OUTPUT / "helper-vfs"
    output.mkdir(exist_ok=True)
    module_path, receipt_path = output / "zxs.so", output / "VFS-BUILD.json"
    module_path.write_bytes(data)
    receipt_path.write_bytes(receipt_bytes)
    return {"module": module_path, "receipt": receipt_path, "module_sha256": checksum,
            "receipt_sha256": hashlib.sha256(receipt_bytes).hexdigest()}


def main():
    root = Path(__file__).resolve().parents[2]
    os.chdir(root)
    receipt = root / OUTPUT / "BUILD.txt"
    # A failed build must not leave an old successful receipt available.
    receipt.unlink(missing_ok=True)
    version = os.environ["RELEASE_VERSION"]
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    require_clean_source()
    require_debian13()
    tree = subprocess.check_output(["git", "rev-parse", "HEAD^{tree}"], text=True).strip()
    web_manifest = root / "build/web/build-manifest.json"
    web_manifest_sha256 = digest(web_manifest)
    build_environment = dict(os.environ, ZXS_EMBED_WEB="1", ZXS_EMBED_WEB_COMMIT=commit,
                             ZXS_EMBED_WEB_TREE=tree)
    # build.rs validates the complete Web input/output inventory and snapshots
    # exact bytes. A missing or stale Web build is never an external-directory fallback.
    # Do not publish arbitrary environment values: compiler/flag overrides may
    # contain private paths or defines. Fingerprints preserve comparison only.
    variables = {key: value for key, value in os.environ.items()
                 if key in {"RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTC", "RUSTC_WRAPPER",
                            "RUSTC_WORKSPACE_WRAPPER", "CC", "CXX", "AR", "CFLAGS", "CXXFLAGS",
                            "ARFLAGS", "CRATE_CC_NO_DEFAULTS"}
                 or key.startswith(("CARGO_BUILD_", "CARGO_TARGET_", "CARGO_PROFILE_RELEASE_", "CC_", "CXX_",
                                    "AR_", "CFLAGS_", "CXXFLAGS_", "ARFLAGS_", "TARGET_", "HOST_"))}
    inputs = {name: digest(root / name) for name in
              ("Cargo.lock", "Cargo.toml", ".cargo/config.toml", "rust-toolchain.toml")}
    compiler = os.environ.get("RUSTC", os.environ.get("CARGO_BUILD_RUSTC", "rustc"))
    rustc = subprocess.check_output([compiler, "-vV"], text=True)
    default_cpu, default_cfg = target_defaults(compiler)
    build_command = ["python3", ".github/scripts/check-rust-diagnostics.py", "--",
                     "cargo", "build", "--locked", "--offline", "--release",
                     "--target", TARGET, "--target-dir", "target/native"]
    # Build ZXS first, then embed exactly that executable into the helper.
    # Never embed a bundle containing the helper itself or download a payload.
    subprocess.run([*build_command, "--package", "storage-server", "--message-format=json"],
                   check=True, env=build_environment)
    payload_path = root / OUTPUT / "zxs"
    payload_sha256 = digest(payload_path)
    materials = legal_materials(root, version, commit, tree, payload_sha256)
    vfs = embedded_vfs(root, commit, tree)
    helper_environment = dict(build_environment, ZXS_HELPER_PAYLOAD=str(payload_path),
                              ZXS_HELPER_PAYLOAD_COMMIT=commit, ZXS_HELPER_PAYLOAD_TREE=tree,
                              ZXS_HELPER_PAYLOAD_VERSION=version,
                              ZXS_HELPER_PAYLOAD_SHA256=payload_sha256,
                              ZXS_HELPER_VFS=str(vfs["module"]),
                              ZXS_HELPER_VFS_SHA256=vfs["module_sha256"],
                              ZXS_HELPER_VFS_RECEIPT=str(vfs["receipt"]),
                              ZXS_HELPER_VFS_RECEIPT_SHA256=vfs["receipt_sha256"],
                              ZXS_HELPER_MATERIALS=str(materials["archive"]),
                              ZXS_HELPER_MATERIALS_SHA256=materials["archive_sha256"],
                              ZXS_HELPER_MATERIALS_MANIFEST=str(materials["manifest"]),
                              ZXS_HELPER_MATERIALS_MANIFEST_SHA256=materials["manifest_sha256"])
    subprocess.run([*build_command, "--package", "storage-helper", "--message-format=json"],
                   check=True, env=helper_environment)
    if digest(payload_path) != payload_sha256:
        raise RuntimeError("embedded helper ZXS payload changed during build")
    if (digest(materials["archive"]) != materials["archive_sha256"]
            or digest(materials["manifest"]) != materials["manifest_sha256"]):
        raise RuntimeError("embedded helper license materials changed during build")
    if (digest(vfs["module"]) != vfs["module_sha256"]
            or digest(vfs["receipt"]) != vfs["receipt_sha256"]):
        raise RuntimeError("embedded helper VFS snapshot changed during build")
    if commit != subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip():
        raise RuntimeError("native release source commit changed during build")
    require_clean_source()
    if inputs != {name: digest(root / name) for name in inputs}:
        raise RuntimeError("native release build configuration changed during build")
    if tree != subprocess.check_output(["git", "rev-parse", "HEAD^{tree}"], text=True).strip():
        raise RuntimeError("native release source tree changed during build")
    if digest(web_manifest) != web_manifest_sha256:
        raise RuntimeError("embedded Web manifest changed during build")
    fields = {
        "native-baseline": "debian13-amd64",
        "native-web-embedded": "true", "native-web-commit": commit,
        "native-web-tree": tree, "native-web-manifest-sha256": web_manifest_sha256,
        "version": version, "commit": commit, "native-target": TARGET,
        "native-profile": "release", "native-rustc": json.dumps(rustc),
        "native-helper-embedded-zxs": "true",
        "native-helper-payload-sha256": payload_sha256,
        "native-helper-embedded-vfs": "true",
        "native-helper-vfs-payload-sha256": vfs["module_sha256"],
        "native-helper-vfs-build-sha256": vfs["receipt_sha256"],
        "native-helper-license-materials-embedded": "true",
        "native-helper-license-materials-sha256": materials["archive_sha256"],
        "native-helper-license-manifest-sha256": materials["manifest_sha256"],
        "native-helper-license-file-count": str(materials["file_count"]),
        "native-helper-license-commit": commit,
        "native-helper-license-tree": tree,
        "native-rust-target-default-cpu": default_cpu,
        "native-rust-target-default-cfg": json.dumps(default_cfg),
        "native-isa-attestation": "not-attested; target-default-query-only; excludes-effective-Rust-C-asm-flags",
        **{key: json.dumps(value) for key, value in override_presence(variables).items()},
        "native-environment-sha256": hashlib.sha256(
            json.dumps(variables, sort_keys=True).encode()).hexdigest(),
        "native-inputs-sha256": hashlib.sha256(
            json.dumps(inputs, sort_keys=True).encode()).hexdigest(),
    }
    for name in BINARIES:
        fields[f"native-{name}-sha256"] = digest(root / OUTPUT / name)
    dependencies = json.dumps(runtime_dependencies(root), sort_keys=True, separators=(",", ":"))
    fields["native-runtime-dependencies"] = dependencies
    fields["native-runtime-dependencies-sha256"] = hashlib.sha256(dependencies.encode()).hexdigest()
    receipt.write_text("".join(f"{key}={value}\n" for key, value in fields.items()))


if __name__ == "__main__":
    main()
