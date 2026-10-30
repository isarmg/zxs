#!/usr/bin/env python3
"""Build the two native release products and record their build-step identity.

This receipt binds selected inputs and output bytes, not full reproducibility,
compiler trust, or the deployment machine's CPU capabilities.
"""
import hashlib
import json
import os
from pathlib import Path
import subprocess

TARGET = "x86_64-unknown-linux-gnu"
OUTPUT = Path("target/native") / TARGET / "release"
BINARIES = ("zxs", "zxs-rescue")


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def main():
    root = Path(__file__).resolve().parents[2]
    os.chdir(root)
    receipt = root / OUTPUT / "BUILD.txt"
    # A failed build must not leave an old successful receipt available.
    receipt.unlink(missing_ok=True)
    version = os.environ["RELEASE_VERSION"]
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
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
    rustc = subprocess.check_output(
        [os.environ.get("RUSTC", os.environ.get("CARGO_BUILD_RUSTC", "rustc")), "-vV"], text=True)
    subprocess.run(["python3", ".github/scripts/check-rust-diagnostics.py", "--",
                    "cargo", "build", "--locked", "--offline", "--release",
                    "--target", TARGET, "--target-dir", "target/native",
                    "--package", "storage-server", "--package", "storage-rescue",
                    "--message-format=json"], check=True)
    if commit != subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip():
        raise RuntimeError("native release source commit changed during build")
    if inputs != {name: digest(root / name) for name in inputs}:
        raise RuntimeError("native release build configuration changed during build")
    fields = {
        "version": version, "commit": commit, "native-target": TARGET,
        "native-profile": "release", "native-rustc": json.dumps(rustc),
        "native-environment-sha256": hashlib.sha256(
            json.dumps(variables, sort_keys=True).encode()).hexdigest(),
        "native-inputs-sha256": hashlib.sha256(
            json.dumps(inputs, sort_keys=True).encode()).hexdigest(),
    }
    for name in BINARIES:
        fields[f"native-{name}-sha256"] = digest(root / OUTPUT / name)
    receipt.write_text("".join(f"{key}={value}\n" for key, value in fields.items()))


if __name__ == "__main__":
    main()
