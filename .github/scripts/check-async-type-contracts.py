#!/usr/bin/env python3
"""Exercise select patterns and public async/guard type boundaries."""
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
TARGET = "x86_64-unknown-linux-gnu"
SELECT_TESTS = {
    "unparenthesized_mut_binding_is_a_control",
    "parenthesized_mut_binding_keeps_the_owned_output",
    "unparenthesized_ref_mut_binding_is_a_control",
    "parenthesized_ref_mut_binding_mutates_only_the_owned_output",
    "parenthesized_nonmatching_pattern_keeps_else_behavior",
}
REJECTIONS = (
    ("select_type", "runtime-select-type-rejection.rs", "E0599"),
    ("select_borrow", "runtime-select-borrow-rejection.rs", "E0716"),
    ("mutex_send", "intrusive-mutex-send-rejection.rs", "E0277"),
)


ARC_RUNS = (
    ("arc_remutex_positive", "arc-remutex-positive.rs", "ARC_REMUTEX_POSITIVE_CONTROLS_COMPLETE"),
    ("arc_remutex_unpin", "arc-remutex-phantom-pinned-unpin.rs", "ARC_REMUTEX_PHANTOM_PINNED_UNPIN_COMPLETE"),
)
ARC_REJECTIONS = (
    ("arc_remutex_cell_sync", "arc-remutex-cell-sync-rejection.rs", "E0277", ("Cell", "Sync")),
    ("arc_remutex_send", "arc-remutex-send-rejection.rs", "E0277", ("Send",)),
)
UPGRADABLE_RUNS = (
    ("upgradable_raw_positive", "upgradable-raw-positive.rs", "UPGRADABLE_POSITIVE_CONTROLS_COMPLETE"),
)
UPGRADABLE_REJECTIONS = (
    ("upgradable_raw_sync", "upgradable-raw-sync-rejection.rs", "E0277", ("Cell", "LocalRaw", "Sync")),
)
GUARD_GROUPS = (
    ("arc_lock", "compile-contracts,arc-contracts", ARC_RUNS, ARC_REJECTIONS),
    ("arc_lock_send_guard", "compile-contracts,arc-contracts,send-guard-contracts", ARC_RUNS, ARC_REJECTIONS),
    ("upgradable_raw", "compile-contracts", UPGRADABLE_RUNS, UPGRADABLE_REJECTIONS),
)


def has_fixture_span(span, fixture):
    """Follow a macro diagnostic back to its actual call site."""
    while span:
        if Path(span["file_name"]) == fixture:
            return True
        expansion = span.get("expansion")
        span = expansion["span"] if expansion else None
    return False


def check_rejection(result, fixture, code, *, fragments=()):
    errors = [
        item["message"]
        for line in result.stdout.splitlines()
        if (item := json.loads(line)).get("reason") == "compiler-message"
        and item["message"]["level"] == "error"
    ]
    if result.returncode != 101 or not errors or any(
        (error.get("code") or {}).get("code") != code
        or not any(
            has_fixture_span(span, fixture)
            for span in error["spans"] if span["is_primary"]
        )
        for error in errors
    ):
        raise AssertionError(
            f"{fixture.name}: expected {code} from this fixture\n"
            + result.stdout + result.stderr
        )
    if fragments and not any(
        all(fragment in json.dumps(error) for fragment in fragments)
        for error in errors
    ):
        raise AssertionError(
            f"{fixture.name}: expected type diagnostic containing {fragments}\n"
            + result.stdout + result.stderr
        )
    print(f"expected compiler rejection: {fixture.name} ({code}): {errors[0]['message']}", flush=True)


def check_main_execution(result, fixture, marker, group):
    """Require a completed fixture main, not an empty test harness."""
    if result.returncode != 0 or result.stdout.splitlines().count(marker) != 1:
        raise AssertionError(
            f"{group}/{fixture.name}: main did not report exactly one {marker}\n"
            + result.stdout + result.stderr
        )
    print(result.stdout, end="", flush=True)
    print(f"runtime-main control: {group}/{fixture.name} passed", flush=True)


def check(root):
    root = Path(root).resolve()
    temporary_root = root / "target/scope-consumers"
    temporary_root.mkdir(parents=True, exist_ok=True)
    fixtures = root / "test/ci/fixtures"
    manifest = f'''[package]
name = "async-type-contracts"
version = "0.0.0"
edition = "2024"
publish = false
autobins = false
autotests = false

[workspace]

[features]
compile-contracts = []
arc-contracts = ["synchronization/arc_lock"]
send-guard-contracts = ["synchronization/send_guard"]

[dependencies]
tokio = {{ path = {json.dumps(str(root / "src/foundation/concurrency/tokio"))}, default-features = false, features = ["macros", "rt"] }}
futures-intrusive = {{ path = {json.dumps(str(root / "src/foundation/concurrency/futures-intrusive"))}, default-features = false, features = ["std"] }}
synchronization = {{ path = {json.dumps(str(root / "src/foundation/concurrency/synchronization"))}, default-features = false, features = ["locks"] }}

[[test]]
name = "select_patterns"
path = {json.dumps(str(fixtures / "runtime-select-patterns.rs"))}

[[bin]]
name = "mutex_send_positive"
path = {json.dumps(str(fixtures / "intrusive-mutex-send-positive.rs"))}
required-features = ["compile-contracts"]
'''
    for name, file, _ in REJECTIONS:
        manifest += f'''
[[bin]]
name = "{name}"
path = {json.dumps(str(fixtures / file))}
required-features = ["compile-contracts"]
'''
    for runs, rejections, features in (
        (ARC_RUNS, ARC_REJECTIONS, ["compile-contracts", "arc-contracts"]),
        (UPGRADABLE_RUNS, UPGRADABLE_REJECTIONS, ["compile-contracts"]),
    ):
        for case in (*runs, *rejections):
            name, file = case[:2]
            manifest += f'''
[[bin]]
name = "{name}"
path = {json.dumps(str(fixtures / file))}
required-features = {json.dumps(features)}
'''
    manifest += '''
[profile.dev]
debug = 0
incremental = false

[profile.test]
debug = 0
incremental = false
'''
    with tempfile.TemporaryDirectory(prefix="async-type-contracts-", dir=temporary_root) as temporary:
        work = Path(temporary)
        (work / "Cargo.toml").write_text(manifest, encoding="utf-8")
        common = ["--manifest-path", str(work / "Cargo.toml"), "--offline"]
        subprocess.run(["cargo", "generate-lockfile", *common], cwd=root, check=True, timeout=120)
        common += [
            "--locked", "--jobs", "1", "--target", TARGET, "--target-dir",
            os.environ.get("CARGO_TARGET_DIR", str(root / "target/async-type-contracts")),
        ]
        tests = ["cargo", "test", *common, "--test", "select_patterns"]
        listing = subprocess.run(tests + ["--", "--list"], cwd=root, check=True,
                                 capture_output=True, text=True, timeout=300)
        names = {line.removesuffix(": test") for line in listing.stdout.splitlines()
                 if line.endswith(": test")}
        if names != SELECT_TESTS:
            raise AssertionError(f"select pattern tests differ: {names}")
        execution = subprocess.run(tests, cwd=root, check=True, capture_output=True,
                                   text=True, timeout=300)
        passed = {line.removeprefix("test ").removesuffix(" ... ok")
                  for line in execution.stdout.splitlines()
                  if line.startswith("test ") and line.endswith(" ... ok")}
        if passed != names or "5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out" not in execution.stdout:
            raise AssertionError("select pattern tests did not all execute\n" + execution.stdout)
        print(execution.stdout, end="", flush=True)
        compile_only = ["cargo", "check", *common, "--features", "compile-contracts"]
        subprocess.run(compile_only + ["--bin", "mutex_send_positive"],
                       cwd=root, check=True, timeout=300)
        print("compile-only control: mutex_send_positive passed", flush=True)
        for name, file, code in REJECTIONS:
            result = subprocess.run(
                compile_only + ["--bin", name, "--message-format=json"],
                cwd=root, capture_output=True, text=True, timeout=300,
            )
            check_rejection(result, fixtures / file, code)

        for group, features, runs, rejections in GUARD_GROUPS:
            for name, file, marker in runs:
                result = subprocess.run(
                    ["cargo", "run", *common, "--features", features, "--bin", name],
                    cwd=root, capture_output=True, text=True, timeout=300,
                )
                check_main_execution(result, fixtures / file, marker, group)
            for name, file, code, fragments in rejections:
                result = subprocess.run(
                    ["cargo", "check", *common, "--features", features,
                     "--bin", name, "--message-format=json"],
                    cwd=root, capture_output=True, text=True, timeout=300,
                )
                check_rejection(result, fixtures / file, code, fragments=fragments)


if __name__ == "__main__":
    check(ROOT)
