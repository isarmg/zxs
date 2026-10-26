#!/usr/bin/env python3
"""Check full runtime utility features and keyed task lifecycle semantics."""
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]


def check(root):
    root = Path(root).resolve()
    temporary_root = root / "target" / "scope-consumers"
    temporary_root.mkdir(parents=True, exist_ok=True)
    runtime = json.dumps(str(root / "src/foundation/concurrency/tokio"))
    service = json.dumps(str(root / "src/foundation/networking/service-stack"))
    futures = json.dumps(str(root / "src/foundation/concurrency/futures"))
    adapters = json.dumps(str(root / "test/ci/fixtures/runtime-adapters.rs"))
    fixture = json.dumps(str(root / "test/ci/fixtures/runtime-join-map.rs"))
    filesystem = json.dumps(str(root / "test/foundation/tokio/tests/fs_asyncify.rs"))
    manifest = f'''[package]
name = "retained-runtime-join-map-consumer"
version = "0.0.0"
edition = "2021"

[workspace]

[dependencies]
tokio = {{ path = {runtime}, features = ["full", "adapters-full"] }}
futures = {{ path = {futures}, default-features = false, features = ["std"] }}
service-stack = {{ path = {service}, default-features = false, features = ["buffer", "limit", "util"] }}

[[test]]
name = "join_map"
path = {fixture}

[[test]]
name = "adapters"
path = {adapters}

[[test]]
name = "fs_asyncify"
path = {filesystem}

[profile.dev]
debug = 0
incremental = false

[profile.test]
debug = 0
incremental = false
'''
    with tempfile.TemporaryDirectory(
        prefix="runtime-join-map-", dir=temporary_root
    ) as temporary:
        work = Path(temporary)
        (work / "Cargo.toml").write_text(manifest, encoding="utf-8")
        common = ["--manifest-path", str(work / "Cargo.toml"), "--offline"]
        subprocess.run(
            ["cargo", "generate-lockfile", *common],
            cwd=root, check=True, timeout=120,
        )
        subprocess.run(
            [
                "cargo", "test", *common, "--locked", "--jobs", "1",
                "--target", "x86_64-unknown-linux-gnu",
                "--target-dir", os.environ.get("CARGO_TARGET_DIR", str(root / "target/runtime-join-map-consumer")),
            ],
            cwd=root, check=True, timeout=300,
        )


if __name__ == "__main__":
    check(ROOT)
