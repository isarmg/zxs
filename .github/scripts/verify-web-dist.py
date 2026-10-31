#!/usr/bin/env python3
"""Verify Web receipts against source-owned inputs and stage exact release bytes."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import stat
import sys

SPEC = "tools/web-build/build-inputs.json"
MANIFEST = "build-manifest.json"
DIGEST = re.compile(r"[0-9a-f]{64}")
REQUIRED_OUTPUTS = {"index.html", "boot.js", "zxs.css", "styles/startup.css",
                    "pkg/zxs.js", "pkg/zxs_bg.wasm", "pkg/zxs_bg.wasm.gz"}


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON field: {key}")
        result[key] = value
    return result


def invalid_constant(value):
    raise ValueError(f"invalid JSON constant: {value}")


def document(data):
    return json.loads(data, object_pairs_hook=unique_object, parse_constant=invalid_constant)


def relative_path(value):
    if (not isinstance(value, str) or not value or "\\" in value or ":" in value
            or any(ord(char) < 32 or ord(char) == 127 for char in value)
            or any(part in ("", ".", "..") for part in value.split("/"))):
        raise ValueError(f"unsafe relative path: {value!r}")
    return value


def input_path(root, relative, directory=False):
    parts = relative_path(relative).split("/")
    path = root
    for index, part in enumerate(parts):
        path /= part
        mode = path.lstat().st_mode
        expected_directory = index < len(parts) - 1 or directory
        if stat.S_ISLNK(mode) or not (stat.S_ISDIR(mode) if expected_directory else stat.S_ISREG(mode)):
            raise ValueError(f"not a regular {'directory' if expected_directory else 'file'}: {path}")
    return path


def read_input(root, relative):
    return input_path(root, relative).read_bytes()


def tree_paths(root, relative):
    directory = input_path(root, relative, directory=True)
    result = []
    for path in sorted(directory.iterdir()):
        name = path.relative_to(root).as_posix()
        mode = path.lstat().st_mode
        if stat.S_ISDIR(mode):
            result.extend(tree_paths(root, name))
        else:
            input_path(root, name)
            result.append(name)
    return result


def digest(data):
    return hashlib.sha256(data).hexdigest()


def source_paths(root):
    spec = document(read_input(root, SPEC))
    if not isinstance(spec, dict) or set(spec) != {"directories", "files"}:
        raise ValueError("invalid Web input specification fields")
    if any(not isinstance(spec[kind], list) for kind in spec):
        raise ValueError("Web input specification paths must be arrays")
    paths = [relative_path(path) for kind in ("directories", "files") for path in spec[kind]]
    if len(set(paths)) != len(paths):
        raise ValueError("duplicate Web input specification path")
    for directory in spec["directories"]:
        if any(path != directory and path.startswith(directory + "/") for path in paths):
            raise ValueError(f"overlapping Web input specification path: {directory}")
    if SPEC not in spec["files"]:
        raise ValueError("Web input specification must inventory itself")
    result = list(spec["files"])
    for directory in spec["directories"]:
        result.extend(tree_paths(root, directory))
    return sorted(result)


def source_inventory(root):
    return {relative: digest(read_input(root, relative)) for relative in source_paths(root)}


def hash_inventory(value, label):
    if not isinstance(value, dict) or not value:
        raise ValueError(f"missing {label} hash inventory")
    for path, checksum in value.items():
        relative_path(path)
        if not isinstance(checksum, str) or not DIGEST.fullmatch(checksum):
            raise ValueError(f"invalid {label} hash: {path}")
    return value


def write_snapshot(root, relative, output, expected=None):
    data = read_input(root, relative)
    if expected is not None and digest(data) != expected:
        raise ValueError(f"content changed while staging: {relative}")
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("xb") as file:
        file.write(data)
    output.chmod(input_path(root, relative).stat().st_mode & 0o777)


def verify_web(root):
    root = Path(root).resolve(strict=True)
    dist = input_path(root, "build/web", directory=True)
    manifest_bytes = read_input(dist, MANIFEST)
    manifest = document(manifest_bytes)
    if (not isinstance(manifest, dict)
            or set(manifest) != {"runtime", "target", "wasm_bindgen", "files", "inputs"}
            or manifest["runtime"] != "web-interface"
            or manifest["target"] != "wasm32-unknown-unknown"
            or manifest["wasm_bindgen"] != "0.2.129"):
        raise ValueError("invalid production Web manifest identity")
    outputs = hash_inventory(manifest["files"], "output")
    inputs = hash_inventory(manifest["inputs"], "input")
    actual_outputs = {path.removeprefix("build/web/") for path in tree_paths(root, "build/web")}
    if actual_outputs != set(outputs) | {MANIFEST} or MANIFEST in outputs:
        raise ValueError("Web output inventory differs from build manifest")
    if not REQUIRED_OUTPUTS <= outputs.keys():
        raise ValueError("production Web distribution lacks required generated assets")
    current_inputs = source_inventory(root)
    if current_inputs != inputs:
        raise ValueError("Web source input inventory/content differs from current checkout; rebuild Web assets")
    for relative, expected in outputs.items():
        if digest(read_input(dist, relative)) != expected:
            raise ValueError(f"Web output checksum mismatch: {relative}")
    return manifest, manifest_bytes


def verify(root, stage=None):
    root = Path(root).resolve(strict=True)
    manifest, manifest_bytes = verify_web(root)
    dist = input_path(root, "build/web", directory=True)
    outputs, inputs = manifest["files"], manifest["inputs"]
    current_inputs = inputs
    # Validate every archive input, not only the files copied by web-build.
    materials = ["README.md", "deploy/systemd/zxs.service", "deploy/samba/smb.conf.example",
                 *tree_paths(root, "compliance"), *tree_paths(root, "docs")]
    for relative in materials:
        input_path(root, relative)
    native_binaries = ("zxs", "zxs-rescue")
    for name in native_binaries:
        binary = input_path(root, f"target/native/x86_64-unknown-linux-gnu/release/{name}")
        if not binary.stat().st_size or not binary.stat().st_mode & 0o111:
            raise ValueError(f"release {name} binary must be nonempty and executable")
    if stage is not None:
        stage = Path(stage)
        # The caller supplies a fresh private directory. Never follow a
        # preexisting stage or an ancestor link while copying release bytes.
        for path in (stage, *stage.absolute().parents):
            if path.is_symlink():
                raise ValueError(f"snapshot symlink: {path}")
        stage.mkdir()
        for relative, expected in outputs.items():
            write_snapshot(dist, relative, stage / "web" / relative, expected)
        if read_input(dist, MANIFEST) != manifest_bytes:
            raise ValueError("Web build manifest changed while staging")
        (stage / "web" / MANIFEST).write_bytes(manifest_bytes)
        for relative in materials:
            write_snapshot(root, relative, stage / "material" / relative, inputs.get(relative))
        for name in native_binaries:
            write_snapshot(root, f"target/native/x86_64-unknown-linux-gnu/release/{name}", stage / "native" / name)
        if source_inventory(root) != current_inputs:
            raise ValueError("Web source inputs changed while staging")
    return len(outputs), len(inputs)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--stage", type=Path)
    args = parser.parse_args()
    try:
        outputs, inputs = verify(args.root, args.stage)
    except (OSError, ValueError, TypeError, KeyError) as error:
        print(f"Release input verification failed: {error}", file=sys.stderr)
        return 1
    print(f"Verified {outputs} Web output hashes and {inputs} current source input hashes.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
