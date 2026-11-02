"""The async ownership/type gate requires intended rejections and completed runtime cases."""
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import tomllib
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "async_type_contracts", ROOT / ".github/scripts/check-async-type-contracts.py"
)
GATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GATE)


def rejection(fixture, code, *, macro=False, description="expected type boundary"):
    span = {"file_name": str(fixture), "is_primary": True, "expansion": None}
    if macro:
        span = {"file_name": "macro-definition.rs", "is_primary": True,
                "expansion": {"span": span}}
    message = {"reason": "compiler-message", "message": {
        "level": "error", "code": {"code": code} if code else None,
        "message": description,
        "spans": [span],
    }}
    return subprocess.CompletedProcess([], 101, json.dumps(message) + "\n", "")


class AsyncTypeContractsGateTests(unittest.TestCase):
    def test_gate_runs_once_in_async_shard(self):
        script = (ROOT / ".github/scripts/check-foundation.sh").read_text()
        shard = script.split("  foundation-async)\n", 1)[1].split(";;", 1)[0]
        self.assertEqual(script.count("check-async-type-contracts.py"), 1)
        self.assertIn("python3 .github/scripts/check-async-type-contracts.py", shard)

    def test_intended_diagnostics_require_fixture_origin_including_macro_expansion(self):
        fixture = Path("/fixture.rs")
        for macro in (False, True):
            with self.subTest(macro=macro):
                GATE.check_rejection(rejection(fixture, "E0716", macro=macro), fixture, "E0716")

    def test_compile_success_and_unrelated_failures_are_rejected(self):
        fixture = Path("/fixture.rs")
        cases = (
            subprocess.CompletedProcess([], 0, "", ""),
            subprocess.CompletedProcess([], 101, "", "dependency build failed"),
            rejection(fixture, "E0432"),
            rejection(Path("/dependency.rs"), "E0277"),
            rejection(Path("/upgradable-raw-support.rs"), "E0277"),
            rejection(Path("/state-receive-owned-support.rs"), "E0277"),
            rejection(Path("/semaphore-raw-transfer-support.rs"), "E0277"),
            rejection(fixture, None),
            subprocess.CompletedProcess([], 1, rejection(fixture, "E0277").stdout, ""),
        )
        for result in cases:
            with self.subTest(result=result), self.assertRaises(AssertionError):
                GATE.check_rejection(result, fixture, "E0277")

    def test_guard_rejections_require_the_intended_type_diagnostic(self):
        fixture = Path("/guard-rejection.rs")
        result = rejection(fixture, "E0277", description="Cell LocalRaw Sync")
        GATE.check_rejection(result, fixture, "E0277", fragments=("Cell", "LocalRaw", "Sync"))
        with self.assertRaises(AssertionError):
            GATE.check_rejection(result, fixture, "E0277", fragments=("Send",))

    def test_owned_state_rejection_requires_the_raw_owner_and_future_chain(self):
        _, file, code, fragments = GATE.OWNED_STATE_REJECTIONS[0]
        fixture = Path("/") / file
        GATE.check_rejection(
            rejection(fixture, "E0277", description="MutexGuard SyncOnlyRaw StateReceiveFuture Send"),
            fixture, code, fragments=fragments,
        )
        for description in (
            "SyncOnlyRaw StateReceiveFuture Send",
            "MutexGuard StateReceiveFuture Send",
            "MutexGuard SyncOnlyRaw Send",
            "MutexGuard SyncOnlyRaw StateReceiveFuture",
        ):
            with self.subTest(description=description), self.assertRaises(AssertionError):
                GATE.check_rejection(
                    rejection(fixture, "E0277", description=description),
                    fixture, code, fragments=fragments,
                )

    def test_semaphore_rejections_require_the_distinct_owner_diagnostics(self):
        required = {
            "semaphore_owned_send": ("MutexGuard", "SyncOnlyRaw", "GenericSharedSemaphoreAcquireFuture", "Send"),
            "semaphore_owner_sync": ("MutexGuard", "SyncOnlyRaw", "GenericSharedSemaphore", "Send"),
        }
        for name, file, code, fragments in GATE.SEMAPHORE_REJECTIONS:
            fixture = Path("/") / file
            chain = required[name]
            GATE.check_rejection(
                rejection(fixture, "E0277", description=" ".join((*chain, "Sync"))),
                fixture, code, fragments=fragments,
            )
            for missing in chain:
                description = " ".join(part for part in (*chain, "Sync") if part != missing)
                with self.subTest(name=name, missing=missing), self.assertRaises(AssertionError):
                    GATE.check_rejection(
                        rejection(fixture, "E0277", description=description),
                        fixture, code, fragments=fragments,
                    )

    def test_runtime_main_requires_success_and_exact_completion_marker(self):
        fixture = Path("/guard-positive.rs")
        marker = "EXPECTED_COMPLETE"
        GATE.check_main_execution(
            subprocess.CompletedProcess([], 0, marker + "\n", ""), fixture, marker, "guard"
        )
        for code, output in (
            (0, ""),
            (0, "test result: ok. 0 passed; 0 failed\n"),
            (0, marker + "\n" + marker + "\n"),
            (1, marker + "\n"),
        ):
            with self.subTest(code=code, output=output), self.assertRaises(AssertionError):
                GATE.check_main_execution(
                    subprocess.CompletedProcess([], code, output, ""), fixture, marker, "guard"
                )

    def run_gate(self, root, *, listed=None, executed=None, main_output=None,
                 test_target="select_patterns"):
        calls = []

        def capture(command, **kwargs):
            manifest = Path(command[command.index("--manifest-path") + 1])
            data = tomllib.loads(manifest.read_text())
            calls.append((command, kwargs, data))
            if "--message-format=json" in command:
                name = command[command.index("--bin") + 1]
                cases = (*GATE.REJECTIONS, *GATE.ARC_REJECTIONS, *GATE.UPGRADABLE_REJECTIONS,
                         *GATE.OWNED_STATE_REJECTIONS, *GATE.SEMAPHORE_REJECTIONS)
                case = next(item for item in cases if item[0] == name)
                _, file, code = case[:3]
                fragments = case[3] if len(case) > 3 else ()
                return rejection(root / "test/ci/fixtures" / file, code,
                                 description="expected type boundary " + " ".join(fragments))
            output = ""
            if command[1] == "test":
                target = command[command.index("--test") + 1]
                expected = {
                    "select_patterns": GATE.SELECT_TESTS,
                    "oneshot_broadcast_receiver_lifecycle": GATE.ONESHOT_LIFECYCLE_TESTS,
                }[target]
                list_output = listed if target == test_target else None
                run_output = executed if target == test_target else None
                if "--list" in command:
                    output = list_output if list_output is not None else "".join(
                        f"{name}: test\n" for name in sorted(expected))
                else:
                    output = run_output if run_output is not None else "".join(
                        f"test {name} ... ok\n" for name in sorted(expected)
                    ) + f"test result: ok. {len(expected)} passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n"
            if command[1] == "run":
                name = command[command.index("--bin") + 1]
                _, _, marker = next(case for case in (*GATE.ARC_RUNS, *GATE.UPGRADABLE_RUNS,
                                                      *GATE.OWNED_STATE_RUNS, *GATE.SEMAPHORE_RUNS)
                                    if case[0] == name)
                output = marker + "\n" if main_output is None else main_output
            return subprocess.CompletedProcess(command, 0, output, "")

        with patch.object(GATE.subprocess, "run", side_effect=capture):
            GATE.check(root)
        return calls

    def test_locked_offline_consumer_preserves_features_and_root_lock(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            lock = root / "Cargo.lock"
            lock.write_text("root lock sentinel\n")
            calls = self.run_gate(root)
            self.assertEqual(lock.read_text(), "root lock sentinel\n")
            self.assertEqual(len(calls), 24)
            original_calls = calls[:21]
            self.assertEqual(len(GATE.ONESHOT_LIFECYCLE_TESTS), 8)
            for command, kwargs, manifest in calls:
                self.assertIn("--offline", command)
                self.assertEqual(kwargs["cwd"], root)
                self.assertGreater(kwargs["timeout"], 0)
                self.assertEqual(manifest["workspace"], {})
                for name, features in (("tokio", ["macros", "rt"]),
                                       ("futures-intrusive", ["std"]), ("synchronization", ["locks"])):
                    dependency = manifest["dependencies"][name]
                    self.assertFalse(dependency["default-features"])
                    self.assertEqual(dependency["features"], features)
                arc_names = {case[0] for case in (*GATE.ARC_RUNS, *GATE.ARC_REJECTIONS)}
                for target in manifest["bin"]:
                    required = ["compile-contracts", "arc-contracts"] if target["name"] in arc_names else ["compile-contracts"]
                    self.assertEqual(target["required-features"], required)
                self.assertEqual(
                    {target["name"]: Path(target["path"]).name for target in manifest["test"]},
                    {
                        "select_patterns": "runtime-select-patterns.rs",
                        "oneshot_broadcast_receiver_lifecycle": "oneshot-broadcast-receiver-lifecycle.rs",
                    },
                )
                self.assertEqual(manifest["features"]["arc-contracts"], ["synchronization/arc_lock"])
                self.assertEqual(manifest["features"]["send-guard-contracts"], ["synchronization/send_guard"])
                if command[1] != "generate-lockfile":
                    self.assertIn("--locked", command)
                    self.assertEqual(command[command.index("--target") + 1], GATE.TARGET)
                if command[1] == "test":
                    self.assertNotIn("--features", command)
                if command[1] in ("check", "run"):
                    name = command[command.index("--bin") + 1]
                    features = command[command.index("--features") + 1]
                    if name in arc_names:
                        self.assertIn(features, {group[1] for group in GATE.GUARD_GROUPS[:2]})
                    else:
                        self.assertEqual(features, "compile-contracts")
            arc_features = (
                "compile-contracts,arc-contracts",
                "compile-contracts,arc-contracts,send-guard-contracts",
            )
            expected_guard_calls = {
                ("run", features, name)
                for features in arc_features
                for name in ("arc_remutex_positive", "arc_remutex_unpin")
            } | {
                ("check", features, name)
                for features in arc_features
                for name in ("arc_remutex_cell_sync", "arc_remutex_send")
            } | {
                ("run", "compile-contracts", "upgradable_raw_positive"),
                ("check", "compile-contracts", "upgradable_raw_sync"),
            }
            actual_guard_calls = {
                (command[1], command[command.index("--features") + 1],
                 command[command.index("--bin") + 1])
                for command, _, _ in calls
                if "--bin" in command
                and command[command.index("--bin") + 1] in {case[2] for case in expected_guard_calls}
            }
            self.assertEqual(actual_guard_calls, expected_guard_calls)
            self.assertEqual(
                [(command[1], command[command.index("--features") + 1],
                  command[command.index("--bin") + 1]) for command, _, _ in original_calls[-4:-2]],
                [("run", "compile-contracts", "state_receive_owned_positive"),
                 ("check", "compile-contracts", "state_receive_owned_send")],
            )
            self.assertEqual(
                [(command[1], command[command.index("--test") + 1], "--list" in command)
                 for command, _, _ in original_calls[-2:]],
                [("test", "oneshot_broadcast_receiver_lifecycle", True),
                 ("test", "oneshot_broadcast_receiver_lifecycle", False)],
            )
            self.assertIn("--test-threads=1", original_calls[-1][0])
            steps = []
            for command, _, _ in calls:
                if "--test" in command:
                    steps.append((command[1], command[command.index("--test") + 1], "--list" in command))
                elif "--bin" in command:
                    steps.append((command[1], command[command.index("--features") + 1],
                                  command[command.index("--bin") + 1]))
                else:
                    steps.append((command[1], None, None))
            self.assertEqual(steps[:21], [
                ("generate-lockfile", None, None),
                ("test", "select_patterns", True),
                ("test", "select_patterns", False),
                ("check", "compile-contracts", "mutex_send_positive"),
                ("check", "compile-contracts", "select_type"),
                ("check", "compile-contracts", "select_borrow"),
                ("check", "compile-contracts", "mutex_send"),
                ("run", "compile-contracts,arc-contracts", "arc_remutex_positive"),
                ("run", "compile-contracts,arc-contracts", "arc_remutex_unpin"),
                ("check", "compile-contracts,arc-contracts", "arc_remutex_cell_sync"),
                ("check", "compile-contracts,arc-contracts", "arc_remutex_send"),
                ("run", "compile-contracts,arc-contracts,send-guard-contracts", "arc_remutex_positive"),
                ("run", "compile-contracts,arc-contracts,send-guard-contracts", "arc_remutex_unpin"),
                ("check", "compile-contracts,arc-contracts,send-guard-contracts", "arc_remutex_cell_sync"),
                ("check", "compile-contracts,arc-contracts,send-guard-contracts", "arc_remutex_send"),
                ("run", "compile-contracts", "upgradable_raw_positive"),
                ("check", "compile-contracts", "upgradable_raw_sync"),
                ("run", "compile-contracts", "state_receive_owned_positive"),
                ("check", "compile-contracts", "state_receive_owned_send"),
                ("test", "oneshot_broadcast_receiver_lifecycle", True),
                ("test", "oneshot_broadcast_receiver_lifecycle", False),
            ])
            self.assertEqual(steps[21:], [
                ("run", "compile-contracts", "semaphore_raw_transfer_positive"),
                ("check", "compile-contracts", "semaphore_owned_send"),
                ("check", "compile-contracts", "semaphore_owner_sync"),
            ])
            self.assertEqual(list((root / "target/scope-consumers").iterdir()), [])

    def test_empty_test_inventory_is_rejected(self):
        for target in ("select_patterns", "oneshot_broadcast_receiver_lifecycle"):
            with self.subTest(target=target), tempfile.TemporaryDirectory() as temporary, \
                    self.assertRaises(AssertionError):
                self.run_gate(Path(temporary), listed="0 tests, 0 benchmarks\n",
                              test_target=target)

    def test_ignored_filtered_or_zero_tests_cannot_count_as_executed(self):
        for target in ("select_patterns", "oneshot_broadcast_receiver_lifecycle"):
            names = sorted(GATE.SELECT_TESTS if target == "select_patterns"
                           else GATE.ONESHOT_LIFECYCLE_TESTS)
            for outcome in ("ignored", "filtered", "zero"):
                lines = [f"test {name} ... ok\n" for name in names[1:]]
                if outcome == "ignored":
                    lines.insert(0, f"test {names[0]} ... ignored\n")
                passed = len(names) - 1
                if outcome == "zero":
                    lines, passed = [], 0
                output = "".join(lines) + (
                    f"test result: ok. {passed} passed; 0 failed; {int(outcome == 'ignored')} ignored; "
                    f"0 measured; {int(outcome == 'filtered')} filtered out\n"
                )
                with self.subTest(target=target, outcome=outcome), \
                        tempfile.TemporaryDirectory() as temporary, self.assertRaises(AssertionError):
                    self.run_gate(Path(temporary), test_target=target, executed=output)

    def test_zero_test_output_cannot_stand_in_for_guard_main(self):
        with tempfile.TemporaryDirectory() as temporary, self.assertRaises(AssertionError):
            self.run_gate(Path(temporary), main_output="test result: ok. 0 passed; 0 failed\n")

    def test_lock_failure_stops_before_checks(self):
        with tempfile.TemporaryDirectory() as temporary:
            with patch.object(GATE.subprocess, "run", side_effect=subprocess.CalledProcessError(1, [])) as run:
                with self.assertRaises(subprocess.CalledProcessError):
                    GATE.check(Path(temporary))
            self.assertEqual(run.call_count, 1)


if __name__ == "__main__":
    unittest.main()
