#!/usr/bin/env python3
"""Exercise public async ownership, pattern, and type contracts."""
if not __debug__:
    raise RuntimeError('Validation requires assertions; do not use Python optimization (-O/-OO or PYTHONOPTIMIZE).')

import json
import os
from pathlib import Path
import runpy
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
BEHAVIOUR = runpy.run_path(str(ROOT / ".github/scripts/check-foundation-behaviour.py"))
TARGET = "x86_64-unknown-linux-gnu"
SELECT_TESTS = {
    "unparenthesized_mut_binding_is_a_control",
    "parenthesized_mut_binding_keeps_the_owned_output",
    "unparenthesized_ref_mut_binding_is_a_control",
    "parenthesized_ref_mut_binding_mutates_only_the_owned_output",
    "parenthesized_nonmatching_pattern_keeps_else_behavior",
}
ONESHOT_LIFECYCLE_TESTS = {
    "broadcast_value_survives_sender_drop_and_fresh_receives",
    "canceled_pending_receive_does_not_close_endpoint",
    "completed_receive_future_repolls_panic",
    "dropping_one_receiver_clone_keeps_survivor_open",
    "last_receiver_drop_closes_with_owned_pending_receive",
    "owned_receive_reads_fulfilled_value_after_endpoint_drop",
    "parallel_receiver_clone_drops_keep_original_open",
    "sender_drop_wakes_pending_receive_as_closed",
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
OWNED_STATE_RUNS = (
    ("state_receive_owned_positive", "state-receive-owned-positive.rs", "STATE_RECEIVE_OWNED_CONTROLS_COMPLETE"),
)
OWNED_STATE_REJECTIONS = (
    ("state_receive_owned_send", "state-receive-owned-send-rejection.rs", "E0277",
     ("MutexGuard", "SyncOnlyRaw", "StateReceiveFuture", "Send")),
)
OWNED_STATE_GROUPS = (
    ("state_receive_owned", "compile-contracts", OWNED_STATE_RUNS, OWNED_STATE_REJECTIONS),
)
SEMAPHORE_RUNS = (
    ("semaphore_raw_transfer_positive", "semaphore-raw-transfer-positive.rs", "SEMAPHORE_RAW_TRANSFER_CONTROLS_COMPLETE"),
)
SEMAPHORE_REJECTIONS = (
    ("semaphore_owned_send", "semaphore-raw-transfer-owned-send-rejection.rs", "E0277",
     ("MutexGuard", "SyncOnlyRaw", "GenericSharedSemaphoreAcquireFuture", "Send")),
    ("semaphore_owner_sync", "semaphore-raw-transfer-owner-sync-rejection.rs", "E0277",
     ("MutexGuard", "SyncOnlyRaw", "GenericSharedSemaphore", "Sync", "Send")),
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

[[test]]
name = "oneshot_broadcast_receiver_lifecycle"
path = {json.dumps(str(fixtures / "oneshot-broadcast-receiver-lifecycle.rs"))}

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
        (OWNED_STATE_RUNS, OWNED_STATE_REJECTIONS, ["compile-contracts"]),
        (SEMAPHORE_RUNS, SEMAPHORE_REJECTIONS, ["compile-contracts"]),
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

        for group, features, runs, rejections in (*GUARD_GROUPS, *OWNED_STATE_GROUPS):
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

        lifecycle = ["cargo", "test", *common, "--test", "oneshot_broadcast_receiver_lifecycle"]
        listing = subprocess.run(lifecycle + ["--", "--list"], cwd=root, check=True,
                                 capture_output=True, text=True, timeout=300)
        names = BEHAVIOUR["listed"](listing.stdout)
        if names != sorted(ONESHOT_LIFECYCLE_TESTS):
            raise AssertionError(f"oneshot lifecycle tests differ: {names}")
        execution = subprocess.run(lifecycle + ["--", "--test-threads=1"],
                                   cwd=root, check=True, capture_output=True,
                                   text=True, timeout=300)
        BEHAVIOUR["check_run"](execution.stdout, names, "oneshot_broadcast_receiver_lifecycle")
        print(execution.stdout, end="", flush=True)

        for name, file, marker in SEMAPHORE_RUNS:
            result = subprocess.run(
                ["cargo", "run", *common, "--features", "compile-contracts", "--bin", name],
                cwd=root, capture_output=True, text=True, timeout=300,
            )
            check_main_execution(result, fixtures / file, marker, "semaphore_raw_transfer")
        for name, file, code, fragments in SEMAPHORE_REJECTIONS:
            result = subprocess.run(
                compile_only + ["--bin", name, "--message-format=json"],
                cwd=root, capture_output=True, text=True, timeout=300,
            )
            check_rejection(result, fixtures / file, code, fragments=fragments)


if __name__ == "__main__":
    check(ROOT)
