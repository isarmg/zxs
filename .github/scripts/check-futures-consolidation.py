#!/usr/bin/env python3
"""Verify the unified futures library, dependency boundaries, and retained behavior."""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile

import runpy

ROOT = Path(__file__).resolve().parents[2]
rebase_dependency_paths = runpy.run_path(str(ROOT / '.github/scripts/foundation_paths.py'))['rebase_dependency_paths']
foundation_path = runpy.run_path(str(ROOT / '.github/scripts/foundation_paths.py'))['package_path']
TARGETS = ("x86_64-unknown-linux-gnu", "wasm32-unknown-unknown")
NATIVE_CONSUMERS = ("storage-client", "storage-server")
FEATURE_SETS = ((), ("alloc",), ("std",), ("sink",), ("channel",),
                ("utilities",), ("utilities", "alloc"), ("utilities", "std"),
                ("io",), ("async-await",), ("async-await-macro",), ("executor",),
                ("thread-pool",), ("shared-state",), ("bilock", "unstable"), ("write-all-vectored",))
ABSORBED = ("futures-core", "futures-channel", "futures-executor", "futures-io",
            "futures-sink", "futures-task", "futures-util")
# These upstream files were already dormant. Keep assertions and source attribution
# intact; do not substitute fake futures-test, assertion, or pin-projection crates.
UNAVAILABLE_TESTS = {'test/foundation/futures/channel/tests/mpsc.rs': ['futures_test'],
 'test/foundation/futures/channel/tests/oneshot.rs': ['futures_test'],
 'test/foundation/futures/tests/_require_features.rs': ['shared feature-requirements helper, not a '
                                                        'test target'],
 'test/foundation/futures/tests/auto_traits.rs': ['static_assertions'],
 'test/foundation/futures/tests/bilock.rs': ['futures_test'],
 'test/foundation/futures/tests/compat.rs': ['tokio',
                                             'removed futures 0.1 / tokio-io compatibility'],
 'test/foundation/futures/tests/compat_miri.rs': ['removed futures 0.1 / tokio-io compatibility'],
 'test/foundation/futures/tests/eager_drop.rs': ['futures_test', 'pin_project'],
 'test/foundation/futures/tests/future_abortable.rs': ['futures_test'],
 'test/foundation/futures/tests/future_basic_combinators.rs': ['futures_test'],
 'test/foundation/futures/tests/future_fuse.rs': ['futures_test'],
 'test/foundation/futures/tests/io_buf_reader.rs': ['futures_test', 'pin_project'],
 'test/foundation/futures/tests/io_buf_writer.rs': ['futures_test'],
 'test/foundation/futures/tests/io_cursor.rs': ['assert_matches'],
 'test/foundation/futures/tests/io_lines.rs': ['futures_test'],
 'test/foundation/futures/tests/io_read.rs': ['futures_test'],
 'test/foundation/futures/tests/io_read_line.rs': ['futures_test'],
 'test/foundation/futures/tests/io_read_to_string.rs': ['futures_test'],
 'test/foundation/futures/tests/io_read_until.rs': ['futures_test'],
 'test/foundation/futures/tests/io_write.rs': ['futures_test'],
 'test/foundation/futures/tests/lock_mutex.rs': ['futures_test'],
 'test/foundation/futures/tests/oneshot.rs': ['futures_test'],
 'test/foundation/futures/tests/ready_queue.rs': ['futures_test'],
 'test/foundation/futures/tests/sink.rs': ['futures_test'],
 'test/foundation/futures/tests/stream.rs': ['futures_test'],
 'test/foundation/futures/tests/stream_abortable.rs': ['futures_test'],
 'test/foundation/futures/tests/stream_futures_ordered.rs': ['futures_test'],
 'test/foundation/futures/tests/stream_futures_unordered.rs': ['futures_test'],
 'test/foundation/futures/tests/stream_into_async_read.rs': ['futures_test'],
 'test/foundation/futures/tests/stream_select_all.rs': ['futures_test'],
 'test/foundation/futures/tests/stream_select_next_some.rs': ['futures_test'],
 'test/foundation/futures/tests/stream_split.rs': ['pin_project'],
 'test/foundation/futures/tests/stream_try_stream.rs': ['futures_test'],
 'test/foundation/futures/tests/stream_unfold.rs': ['futures_test'],
 'test/foundation/futures/tests/test_macro.rs': ['futures_test']}


def run(root, args, *, succeeds=True):
    result = subprocess.run(args, cwd=root, capture_output=True, text=True, timeout=600)
    if succeeds and result.returncode:
        raise RuntimeError(" ".join(map(str, args)) + "\n" + result.stdout + result.stderr)
    if not succeeds and not result.returncode:
        raise AssertionError("negative futures probe unexpectedly compiled")
    return result


def validate_dependencies(names, features):
    features = set(features)
    if names.intersection(ABSORBED):
        raise AssertionError("absorbed futures package remains in the compiled graph")
    if not features:
        if names != {"futures", "futures-probe"}:
            raise AssertionError(f"bare traits acquired dependencies: {names}")
    if features <= {"alloc", "std", "sink", "channel"}:
        if names.intersection({"slab", "pin-project-lite", "futures-macro"}):
            raise AssertionError(f"primitive feature acquired utility or macro dependencies: {names}")
    if "shared-state" not in features and "slab" in names:
        raise AssertionError("shared-state storage was enabled without an explicit request")
    if "async-await-macro" not in features and "futures-macro" in names:
        raise AssertionError("procedural macros were enabled without an explicit request")


def validate_native_consumer_features(output, package):
    features = [set(match.group(1).split(',')) for line in output.splitlines()
                if (match := re.match(r'^futures v[^|]*\|([^\s]*)', line))]
    forbidden = {"async-await", "async-await-macro", "shared-state"}
    if not features or any(forbidden & selected for selected in features):
        raise AssertionError(f"{package} futures acquired unused capabilities: {features}")
    if any(line.startswith("futures-macro v") for line in output.splitlines()):
        raise AssertionError(f"{package} acquired unused futures procedural macros")


def check_native_consumer_features(root):
    # Select each real consumer separately: a workspace-wide graph would let
    # browser or sibling-package features hide an accidental native dependency.
    for package in NATIVE_CONSUMERS:
        for edges in ("normal,build", "normal,build,dev"):
            result = run(root, ["cargo", "tree", "--locked", "--offline", "-p", package,
                               "--target", TARGETS[0], "--edges", edges,
                               "--prefix", "none", "--format", "{p}|{f}"])
            validate_native_consumer_features(result.stdout, package)


def check_features(root, temporary, target_dir):
    for index, features in enumerate(FEATURE_SETS):
        work = temporary / f"minimal-{index}"
        (work / "src").mkdir(parents=True)
        source = work / "src/lib.rs"
        source.write_text(
            "#![no_std]\n"
            "pub fn fused<F: futures::future::FusedFuture>(f: &F) -> bool { f.is_terminated() }\n"
            "pub fn sink<S: futures::Sink<u8>>() {}\n"
            "pub fn stream<S: futures::Stream>() {}\n"
        )
        (work / "Cargo.toml").write_text(
            '[package]\nname="futures-probe"\nversion="0.0.0"\nedition="2021"\n'
            '[workspace]\nresolver="2"\n[dependencies.futures]\n'
            f'path={json.dumps(str(root / "src/foundation/concurrency/futures"))}\n'
            f'default-features=false\nfeatures={json.dumps(features)}\n'
            '[profile.dev]\ndebug=0\nincremental=false\n'
        )
        common = ["--offline", "--manifest-path", str(work / "Cargo.toml")]
        run(root, ["cargo", "generate-lockfile", *common])
        # cargo metadata's all-platform resolution can report weak optional edges
        # that are not compiled. cargo tree reports the selected compilation graph.
        tree = run(root, ["cargo", "tree", *common, "--locked", "--prefix", "none", "--format", "{p}"])
        validate_dependencies({line.split()[0] for line in tree.stdout.splitlines() if line}, features)
        for target in TARGETS:
            command = ["cargo", "check", *common, "--locked", "--target", target,
                       "--target-dir", str(target_dir), "--jobs", "2"]
            run(root, command)
        if not features:
            for missing in ("FutureExt", "executor::block_on", "join"):
                source.write_text(f"use futures::{missing};\n")
                result = run(root, command, succeeds=False)
                if "E0432" not in result.stderr:
                    raise AssertionError("feature-negative probe failed for an unrelated reason")
        if features == ("utilities", "std"):
            source.write_text("use futures::FutureExt;\npub fn shared() { let _ = futures::future::ready(1).shared(); }\n")
            result = run(root, command, succeeds=False)
            if "E0599" not in result.stderr or "shared" not in result.stderr:
                raise AssertionError("shared-state negative probe failed for an unrelated reason")
        if features == ("async-await-macro",):
            manifest = (work / "Cargo.toml").read_text().replace(
                "[dependencies.futures]", '[dependencies.async-primitives]\npackage="futures"')
            (work / "Cargo.toml").write_text(manifest)
            source.write_text(
                "#![no_std]\n"
                "pub async fn join() -> (u8, u8) { async_primitives::join!(async { 1 }, async { 2 }) }\n"
                "pub async fn choose() -> u8 { async_primitives::select_biased! {\n"
                "x = async_primitives::future::ready(3) => x } }\n"
            )
            run(root, command)
    # Full retained feature union is independently compiled on both supported targets.
    all_features = temporary / "minimal-15" / "Cargo.toml"
    for target in TARGETS:
        run(root, ["cargo", "check", "--locked", "--offline", "--manifest-path", str(all_features),
                   "--features", "futures/alloc,futures/std,futures/utilities,futures/sink,futures/channel,futures/io,futures/async-await-macro,futures/executor,futures/thread-pool,futures/shared-state,futures/bilock,futures/unstable,futures/write-all-vectored",
                   "--target", target, "--target-dir", str(target_dir), "--jobs", "2"])


def validate_retained_output(output, integration_targets):
    summaries = re.findall(r"test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored", output)
    if (len(summaries) < integration_targets + 2
            or int(summaries[0][0]) < 8
            or sum(int(row[0]) for row in summaries[1:-1]) < 124
            or int(summaries[-1][0]) < 203):
        raise AssertionError("retained futures tests or doctests did not execute completely")
    if any(int(row[1]) for row in summaries):
        raise AssertionError("retained futures test failures were reported")


def check_retained_tests(root, temporary, target_dir):
    mirror = temporary / "retained"
    package = mirror / "src/foundation/concurrency/futures"
    shutil.copytree(root / "src/foundation/concurrency/futures", package)
    shutil.copytree(root / "test/foundation/futures", mirror / "test/foundation/futures")
    manifest = (package / "Cargo.toml").read_text()
    manifest = rebase_dependency_paths(manifest, root / "src/foundation/concurrency/futures")
    manifest += '\n[workspace]\nresolver="2"\n[profile.dev]\ndebug=0\nincremental=false\n[profile.test]\ndebug=0\nincremental=false\n'
    found_unavailable = set()
    count = 0
    for test in sorted((mirror / "test/foundation/futures").rglob("tests/*.rs")):
        relative = test.relative_to(mirror).as_posix()
        if relative in UNAVAILABLE_TESTS:
            found_unavailable.add(relative)
            continue
        name = test.relative_to(mirror / "test/foundation/futures").as_posix().replace("/", "_").removesuffix(".rs").replace("-", "_")
        manifest += f'\n[[test]]\nname={json.dumps(name)}\npath={json.dumps("../../../../" + relative)}\n'
        count += 1
    if found_unavailable != set(UNAVAILABLE_TESTS):
        raise AssertionError("dormant upstream test inventory drifted")
    (package / "Cargo.toml").write_text(manifest)
    run(root, ["cargo", "generate-lockfile", "--offline", "--manifest-path", str(package / "Cargo.toml")])
    result = run(root, ["cargo", "test", "--locked", "--offline", "--manifest-path", str(package / "Cargo.toml"),
                       "--all-features", "--target-dir", str(target_dir), "--jobs", "2"])
    validate_retained_output(result.stdout, count)
    print(result.stdout)
    print(f"Verified {count} retained integration targets; {len(found_unavailable)} explicitly unavailable upstream files remain preserved.")


def check(root):
    root = Path(root).resolve()
    check_native_consumer_features(root)
    for name in ABSORBED:
        for path in (foundation_path(root, name), root / "test/foundation" / name):
            if path.exists():
                raise AssertionError(f"absorbed source root remains: {path.relative_to(root)}")
    temporary_root = root / "target/scope-consumers"
    temporary_root.mkdir(parents=True, exist_ok=True)
    target_dir = Path(os.environ.get("CARGO_TARGET_DIR", root / "target/futures-consumers"))
    with tempfile.TemporaryDirectory(prefix="futures-", dir=temporary_root) as directory:
        temporary = Path(directory)
        check_features(root, temporary, target_dir)
        runpy.run_path(str(root / '.github/scripts/futures_io_contracts.py'))['check'](
            root, temporary / 'io-contracts', target_dir
        )
        check_retained_tests(root, temporary, target_dir)
    for package in (*NATIVE_CONSUMERS, "platform-io"):
        run(root, ["cargo", "check", "--locked", "--offline", "--jobs", "2", "--target-dir", str(target_dir),
                   "--target", TARGETS[0], "--all-targets", "-p", package])
    run(root, ["cargo", "check", "--locked", "--offline", "--jobs", "2", "--target-dir", str(target_dir),
               "-p", "web-interface", "--target", "wasm32-unknown-unknown"])


if __name__ == "__main__":
    check(ROOT)
