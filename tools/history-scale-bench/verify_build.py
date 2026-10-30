#!/usr/bin/env python3
"""Verify actual Cargo compiler-artifact features, not only a planned tree."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

from summarize import read_json


FORBIDDEN = frozenset(("fault-injection", "test-support"))
ADAPTER_FEATURES = {"baseline": "history-scale-baseline", "current": "history-scale-current"}


def digest(path: Path) -> str:
    state = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            state.update(block)
    return state.hexdigest()


def required_digest(value: object) -> str:
    if not isinstance(value, str) or re.fullmatch(r"[0-9a-f]{64}", value) is None:
        raise ValueError("expected a lowercase SHA-256 digest")
    return value


def verify(lines) -> dict:
    selected = {}
    finished = False
    for line in lines:
        message = read_json(line)
        reason = message["reason"]
        if finished:
            raise ValueError("Cargo emitted records after build completion")
        if reason == "compiler-message":
            diagnostic = message["message"]
            if diagnostic["level"] in {"error", "warning"}:
                raise ValueError(f"compiler {diagnostic['level']}: {diagnostic['message']}")
        elif reason == "build-finished":
            if not message["success"]:
                raise ValueError("Cargo reported an unsuccessful build")
            finished = True
        elif reason == "compiler-artifact":
            target = message["target"]
            name = target["name"]
            if name not in {"storage_engine", "history-scale-driver"}:
                continue
            features = message["features"]
            if not isinstance(features, list) or any(not isinstance(feature, str) for feature in features) or len(set(features)) != len(features):
                raise ValueError(f"invalid actual feature list for {name}")
            if FORBIDDEN.intersection(features):
                raise ValueError(f"forbidden actual features on {name}: {features}")
            profile = message["profile"]
            if profile["test"] or profile["opt_level"] != "3" or profile["debug_assertions"]:
                raise ValueError(f"expected ordinary release artifact for {name}: {profile}")
            expected_kind = "lib" if name == "storage_engine" else "bin"
            if target["kind"] != [expected_kind]:
                raise ValueError(f"unexpected target kind for {name}: {target['kind']}")
            if name in selected:
                raise ValueError(f"duplicate actual artifact for {name}")
            # The empty feature list is accepted only for read-only inspection of
            # archived candidate builds. Fresh provenance requires one adapter.
            if name == "history-scale-driver" and features not in [[], *[[feature] for feature in ADAPTER_FEATURES.values()]]:
                raise ValueError("driver must select exactly one audited history adapter")
            selected[name] = {
                key: message[key]
                for key in ("package_id", "manifest_path", "features", "profile", "target", "fresh", "executable", "filenames")
            }
    if not finished or set(selected) != {"storage_engine", "history-scale-driver"}:
        raise ValueError("missing successful completion or actual engine/driver artifacts")
    return {"verified": True, "artifacts": selected}


def expected_command(adapter: str) -> list[str]:
    return ["cargo", "+1.99.0", "build", "--locked", "--offline", "--release", "-p", "perf-check",
            "--bin", "history-scale-driver", "--features", ADAPTER_FEATURES[adapter], "--message-format=json"]


def command_output(command: list[str], source: Path | None = None) -> str:
    return subprocess.check_output(command, cwd=source, text=True, stderr=subprocess.STDOUT).strip()


def registration_binding(registration: dict, source: Path, binary: Path, artifacts: dict, command: list) -> dict:
    adapter = registration["adapter"]
    if adapter not in ADAPTER_FEATURES or type(registration.get("schema_version")) is not int or registration["schema_version"] != 2:
        raise ValueError("unsupported registration adapter/schema")
    if command != expected_command(adapter):
        raise ValueError("actual build command does not match the audited ordinary release invocation")
    sha = command_output(["git", "rev-parse", "HEAD"], source)
    if sha != registration["source_sha"]:
        raise ValueError("registration source does not match checkout HEAD")
    changed = command_output(["git", "diff", "HEAD", "--name-only"], source).splitlines()
    untracked = command_output(["git", "ls-files", "--others", "--exclude-standard"], source).splitlines()
    placement = registration["driver_registration"]
    if placement not in {"tracked", "copied"}:
        raise ValueError("unknown driver registration placement")
    tracked = command_output(["git", "ls-files", "--", "tools/history-scale-bench/driver.rs"], source)
    expected_untracked = [] if placement == "tracked" else ["tools/history-scale-bench/driver.rs"]
    if bool(tracked) != (placement == "tracked") or changed != ["tools/perf-check/Cargo.toml"] or untracked != expected_untracked:
        raise ValueError("source changed outside the audited temporary harness registration")
    driver = source / "tools/history-scale-bench/driver.rs"
    manifest = source / "tools/perf-check/Cargo.toml"
    original = subprocess.check_output(["git", "show", "HEAD:tools/perf-check/Cargo.toml"], cwd=source)
    checks = {
        "driver_sha256": digest(driver), "lockfile_sha256": digest(source / "Cargo.lock"),
        "manifest_sha256_before": hashlib.sha256(original).hexdigest(), "manifest_sha256_after": digest(manifest),
    }
    for name, actual in checks.items():
        if required_digest(registration[name]) != actual:
            raise ValueError(f"registration {name} mismatch")
    if registration.get("build_features") != [ADAPTER_FEATURES[adapter]]:
        raise ValueError("registration feature does not match adapter")
    actual = artifacts["artifacts"]
    if actual["history-scale-driver"]["features"] != registration["build_features"]:
        raise ValueError("compiled adapter does not match registration")
    for name, relative in (("storage_engine", "src/engine/Cargo.toml"), ("history-scale-driver", "tools/perf-check/Cargo.toml")):
        if Path(actual[name]["manifest_path"]).resolve() != source / relative:
            raise ValueError(f"actual {name} artifact came from another source manifest")
    if Path(actual["history-scale-driver"]["target"]["src_path"]).resolve() != driver:
        raise ValueError("Cargo compiled a different driver source")
    executable = Path(actual["history-scale-driver"]["executable"]).resolve(strict=True)
    if executable != source / "target/native/release/history-scale-driver":
        raise ValueError("unexpected Cargo driver executable target")
    if digest(executable) != digest(binary):
        raise ValueError("bundled binary differs from Cargo's actual executable")
    return {"source_sha": sha, "adapter": adapter, **checks}


def create_provenance(arguments: argparse.Namespace, verified: dict) -> dict:
    if os.environ.get("GITHUB_ACTIONS") != "true":
        raise ValueError("formal build provenance is generated only by GitHub Actions")
    source = arguments.source.resolve(strict=True)
    binary = arguments.binary.resolve(strict=True)
    registration = read_json(arguments.registration.read_text(encoding="utf-8"))
    command = read_json(arguments.build_command.read_text(encoding="utf-8"))
    binding = registration_binding(registration, source, binary, verified, command)
    rustc = command_output(["rustc", "+1.99.0", "--version", "--verbose"])
    cargo = command_output(["cargo", "+1.99.0", "--version"])
    if rustc.splitlines()[0].split()[1] != "1.99.0" or cargo.split()[1] != "1.99.0":
        raise ValueError("precompiled provenance requires Rust/Cargo 1.99.0")
    return {
        "schema_version": 1, "verified": True, **binding,
        "binary_name": "history-scale-driver", "binary_sha256": digest(binary),
        "cargo_config_sha256": digest(source / ".cargo/config.toml"),
        "toolchain_config_sha256": digest(source / "rust-toolchain.toml"),
        "compiler_jsonl_sha256": digest(arguments.input), "compiler_jsonl_file": arguments.input.name,
        "registration_sha256": digest(arguments.registration), "registration_file": arguments.registration.name,
        "build_command": command, "registration": registration, "actual_artifacts": verified,
        "rustc_verbose": rustc, "cargo_version": cargo,
        "build_identity": {key: os.environ[key] for key in ("GITHUB_SHA", "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT")},
        "isa": {
            "rustflags": os.environ.get("RUSTFLAGS", ""),
            "encoded_rustflags": os.environ.get("CARGO_ENCODED_RUSTFLAGS", ""),
            "build_target": os.environ.get("CARGO_BUILD_TARGET", ""),
            "cpu_info": read_json(command_output(["lscpu", "--json"])),
            "compiler_host": next(line.removeprefix("host: ") for line in rustc.splitlines() if line.startswith("host: ")),
        },
    }


def validate_precompiled(provenance_path: Path, binding: dict) -> dict:
    proof = read_json(provenance_path.read_text(encoding="utf-8"))
    if not isinstance(proof, dict) or type(proof.get("schema_version")) is not int or proof["schema_version"] != 1 or proof.get("verified") is not True:
        raise ValueError("invalid precompiled binary provenance schema")
    fields = {"schema_version", "verified", "source_sha", "adapter", "driver_sha256", "lockfile_sha256",
              "manifest_sha256_before", "manifest_sha256_after", "binary_name", "binary_sha256",
              "cargo_config_sha256", "toolchain_config_sha256", "compiler_jsonl_sha256", "compiler_jsonl_file",
              "registration_sha256", "registration_file", "build_command", "registration", "actual_artifacts",
              "rustc_verbose", "cargo_version", "isa", "build_identity"}
    if set(proof) != fields:
        raise ValueError("incomplete or unknown precompiled provenance fields")
    required_binding = {"source_sha", "adapter", "binary_name", "binary_sha256", "driver_sha256", "lockfile_sha256",
                        "manifest_sha256_before", "cargo_config_sha256", "toolchain_config_sha256"}
    if set(binding) != required_binding:
        raise ValueError("complete source/binary/driver/manifest provenance binding is required")
    for key, expected in binding.items():
        if proof.get(key) != expected:
            raise ValueError(f"precompiled binary {key} mismatch")
    for key in ("binary_sha256", "driver_sha256", "lockfile_sha256", "manifest_sha256_before", "manifest_sha256_after", "compiler_jsonl_sha256", "registration_sha256", "cargo_config_sha256", "toolchain_config_sha256"):
        required_digest(proof.get(key))
    adapter = proof.get("adapter")
    if adapter not in ADAPTER_FEATURES or proof.get("build_command") != expected_command(adapter):
        raise ValueError("invalid precompiled adapter/build command")
    if proof["rustc_verbose"].splitlines()[0].split()[1] != "1.99.0" or proof["cargo_version"].split()[1] != "1.99.0":
        raise ValueError("precompiled compiler/toolchain mismatch")
    isa = proof["isa"]
    if not isinstance(isa, dict) or set(isa) != {"rustflags", "encoded_rustflags", "build_target", "cpu_info", "compiler_host"}:
        raise ValueError("missing precompiled ISA/compiler-host evidence")
    if any(not isinstance(isa[key], str) for key in ("rustflags", "encoded_rustflags", "build_target", "compiler_host")) or not isinstance(isa["cpu_info"], dict):
        raise ValueError("invalid precompiled ISA evidence")
    if "host: " + isa["compiler_host"] not in proof["rustc_verbose"].splitlines():
        raise ValueError("precompiled compiler host binding mismatch")
    identity = proof["build_identity"]
    if not isinstance(identity, dict) or set(identity) != {"GITHUB_SHA", "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT"}:
        raise ValueError("missing build carrier/run identity")
    if not isinstance(identity["GITHUB_SHA"], str) or re.fullmatch(r"[0-9a-f]{40}", identity["GITHUB_SHA"]) is None:
        raise ValueError("invalid build carrier SHA")
    if any(not isinstance(identity[key], str) or not identity[key].isdigit() or int(identity[key]) < 1 for key in ("GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT")):
        raise ValueError("invalid build run identity")
    evidence = {}
    for kind in ("compiler_jsonl", "registration"):
        filename = proof.get(kind + "_file")
        if not isinstance(filename, str) or re.fullmatch(r"[A-Za-z0-9_.-]+", filename) is None or filename in {".", ".."}:
            raise ValueError(f"invalid {kind} evidence filename")
        path = provenance_path.parent / filename
        if digest(path) != proof[kind + "_sha256"]:
            raise ValueError(f"precompiled {kind} evidence digest mismatch")
        evidence[kind] = path
    with evidence["compiler_jsonl"].open(encoding="utf-8") as source:
        actual = verify(source)
    if actual != proof["actual_artifacts"] or actual["artifacts"]["history-scale-driver"]["features"] != [ADAPTER_FEATURES[adapter]]:
        raise ValueError("precompiled compiler artifacts/adapter mismatch")
    registration = read_json(evidence["registration"].read_text(encoding="utf-8"))
    if registration != proof["registration"]:
        raise ValueError("precompiled registration content mismatch")
    if type(registration.get("schema_version")) is not int or registration["schema_version"] != 2 or registration.get("driver_registration") not in {"tracked", "copied"}:
        raise ValueError("unsupported tracked/copied driver registration evidence")
    for key in ("source_sha", "adapter", "driver_sha256", "lockfile_sha256", "manifest_sha256_before", "manifest_sha256_after"):
        if registration.get(key) != proof[key]:
            raise ValueError(f"precompiled registration binding mismatch: {key}")
    if registration.get("build_features") != [ADAPTER_FEATURES[adapter]]:
        raise ValueError("precompiled registration feature mismatch")
    return proof


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--source", type=Path)
    parser.add_argument("--binary", type=Path)
    parser.add_argument("--registration", type=Path)
    parser.add_argument("--build-command", type=Path)
    parser.add_argument("--output", type=Path)
    arguments = parser.parse_args()
    with arguments.input.open() as source:
        verified = verify(source)
    supplied = (arguments.source, arguments.binary, arguments.registration, arguments.build_command, arguments.output)
    if any(value is not None for value in supplied) and any(value is None for value in supplied):
        parser.error("provenance generation requires source, binary, registration, build-command and output together")
    result = create_provenance(arguments, verified) if arguments.source is not None else verified
    if arguments.output is not None:
        with arguments.output.open("x", encoding="utf-8") as destination:
            json.dump(result, destination, indent=2, sort_keys=True)
            destination.write("\n")
    print(json.dumps(result, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
