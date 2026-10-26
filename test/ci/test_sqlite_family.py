"""SQLite executable coverage stays wired to real tests and isolated Cargo locks."""
import contextlib
import importlib.util
import io
import os
from pathlib import Path
import subprocess
import tempfile
import tomllib
import unittest
from unittest.mock import patch

import runpy

ROOT = Path(__file__).resolve().parents[2]
foundation_path = runpy.run_path(str(ROOT / '.github/scripts/foundation_paths.py'))['package_path']
SPEC = importlib.util.spec_from_file_location("sqlite_family", ROOT / ".github/scripts/check-sqlite-family.py")
module = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(module)


class SqliteFamily(unittest.TestCase):
    def test_foundation_gate_is_wired_after_root_lock_units(self):
        script = (ROOT / ".github/scripts/check-foundation.sh").read_text()
        self.assertIn("cargo test --locked --offline --lib -p sqlite-access\n    python3 .github/scripts/check-sqlite-family.py", script)

    def test_all_executable_commands_require_existing_offline_lock(self):
        for action in ("test", "check"):
            command = module.cargo_command(action, Path("Cargo.toml"), Path("target"), "--lib")
            self.assertEqual(command[:4], ["cargo", action, "--locked", "--offline"])
            self.assertIn("--manifest-path", command)
            self.assertIn("--target-dir", command)

    def test_captured_commands_override_forced_color(self):
        with patch.dict(os.environ, {"CARGO_TERM_COLOR": "always"}):
            command = module.cargo_command("check", Path("Cargo.toml"), Path("target"))
            self.assertEqual(command[command.index("--color") + 1], "never")
            self.assertEqual(os.environ["CARGO_TERM_COLOR"], "always")

    def test_negative_matches_colored_diagnostic_without_weakening_failure(self):
        diagnostic = "\x1b[91merror[E0603]\x1b[0m: private module"
        for status in (0, 1):
            with patch.object(module.subprocess, "run", return_value=subprocess.CompletedProcess([], status, "", diagnostic)):
                if status:
                    module.execute(["cargo"], ROOT, expected="error[E0603]")
                else:
                    with self.assertRaises(AssertionError):
                        module.execute(["cargo"], ROOT, expected="error[E0603]")

    def test_existing_integration_paths_are_not_replaced_by_fixtures(self):
        self.assertEqual(module.INTEGRATION_TESTS, {
            "hash_table_compatibility": "test/engine/integration/hash_table_compatibility.rs",
            "sqlite_foundation": "test/platform/integration/sqlite_foundation.rs",
        })
        self.assertTrue(all((ROOT / path).is_file() for path in module.INTEGRATION_TESTS.values()))

    def test_consumer_uses_actual_provider_paths_and_explicit_features(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = module.consumer_manifest(ROOT, Path(temporary), [])
            manifest = tomllib.loads(path.read_text())
            dependencies = manifest["dependencies"]
            self.assertEqual(dependencies["sqlite-access"]["features"], [])
            self.assertFalse(dependencies["sqlite-access"]["default-features"])
            self.assertEqual(set(dependencies), {"sqlite-access"})
            self.assertEqual(manifest["features"]["sqlite-pool"], ["sqlite-access/pool"])
            for name, entry in {**dependencies, **manifest["dev-dependencies"]}.items():
                self.assertEqual(Path(entry["path"]), foundation_path(ROOT, name))
            self.assertIn("workspace", manifest)
            self.assertEqual({row["path"] for row in manifest["test"]},
                             {str(ROOT / path) for path in module.INTEGRATION_TESTS.values()})

    def test_consumer_declares_shared_regression_dependencies_only_for_tests(self):
        for features in ([], ["sqlite-bundled"]):
            with self.subTest(features=features), tempfile.TemporaryDirectory() as temporary:
                path = module.consumer_manifest(ROOT, Path(temporary), features)
                manifest = tomllib.loads(path.read_text())
                self.assertEqual(set(manifest["dependencies"]), {"sqlite-access"})
                self.assertEqual(manifest["dependencies"]["sqlite-access"]["features"], features)
                development = manifest["dev-dependencies"]
                for name in ("tempfile", "log", "futures", "libsqlite3-sys"):
                    self.assertEqual(Path(development[name]["path"]), foundation_path(ROOT, name))
                self.assertEqual(development["log"]["features"], ["std"])
                self.assertFalse(development["futures"]["default-features"])
                self.assertEqual(development["futures"]["features"], ["std", "utilities"])
                self.assertFalse(development["libsqlite3-sys"]["default-features"])
                self.assertNotIn("features", development["libsqlite3-sys"])

    def test_direct_graph_excludes_test_dependency_edges(self):
        direct = "consumer v0.0.0|\nsqlite-access v0.9.0|sqlite-bundled\nurl-encoding v2.5.8|alloc,form,std\n"
        with patch.object(module.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, direct, "")) as run:
            module.check_direct_dependencies(Path("Cargo.toml"), ROOT)
        command = run.call_args.args[0]
        self.assertEqual(command[command.index("--edges") + 1], "normal,build")

    def test_disposable_package_keeps_tests_but_uses_real_dependency_providers(self):
        with tempfile.TemporaryDirectory() as temporary:
            work = Path(temporary)
            manifest = module.prepare_package(ROOT, work)
            parsed = tomllib.loads(manifest.read_text())
            self.assertEqual(parsed["lib"]["path"], "src/lib.rs")
            self.assertIn("workspace", parsed)
            for entry in parsed["dependencies"].values():
                if isinstance(entry, dict) and "path" in entry:
                    self.assertTrue(Path(entry["path"]).is_relative_to(ROOT / "src/foundation"))
            self.assertEqual(module.FAMILY_TEST_ROOTS,
                             ("test/foundation/sqlite-access", "test/platform/unit/sqlite/driver"))
            for relative in module.FAMILY_TEST_ROOTS:
                for path in (ROOT / relative).rglob("*"):
                    if path.is_file():
                        self.assertEqual(path.read_bytes(), (work / path.relative_to(ROOT)).read_bytes())
            self.assertFalse((manifest.parent / "Cargo.lock").exists())

    def test_transaction_owns_the_original_source_without_a_pool_wrapper(self):
        query_engine = ROOT / "src/foundation/data-storage/sqlite-access/src/query_engine"
        transaction = (query_engine / "transaction.rs").read_text()
        self.assertIn('#[path = "pool/maybe.rs"]\nmod connection;', transaction)
        self.assertTrue((query_engine / "pool/maybe.rs").is_file())
        self.assertFalse((query_engine / "transaction/connection.rs").exists())
        self.assertNotIn("mod maybe;", (query_engine / "pool/mod.rs").read_text())
        implementation = (query_engine / "pool/maybe.rs").read_text()
        self.assertIn("pub enum TransactionConnection", implementation)
        self.assertNotIn("MaybePoolConnection", implementation)

    def test_gate_preserves_maintained_sources_and_root_lock(self):
        # Exercise every file mutation and the finally assertions without executing Cargo.
        with patch.object(module, "execute") as execute, patch.object(module, "check_direct_dependencies"), \
                contextlib.redirect_stdout(io.StringIO()):
            module.check(ROOT)
        doctests = [call for call in execute.call_args_list if "--doc" in call.args[0]]
        self.assertEqual(len(doctests), 1)
        self.assertEqual(doctests[0].args[0][-4:],
                         ["--doc", "--features", "sqlite-bundled", "types::Type"])
        self.assertEqual(doctests[0].kwargs["minimum_tests"], 1)
        platform_tests = [call.args[0] for call in execute.call_args_list
                          if call.args[0][:2] == ["cargo", "test"] and "platform-io" in call.args[0]]
        self.assertEqual(len(platform_tests), 2)
        for command in platform_tests:
            self.assertEqual(command[-3:], ["sqlite::", "--", "--test-threads=1"])

    def test_direct_graph_rejects_optional_runtime_and_unicode_dependencies(self):
        direct = "consumer v0.0.0|\nsqlite-access v0.9.0|sqlite-bundled\nurl-encoding v2.5.8|alloc,form,std\n"
        module.assert_direct_dependencies(direct)
        for package in module.DIRECT_EXCLUDED_PACKAGES:
            with self.assertRaises(AssertionError):
                module.assert_direct_dependencies(direct + f"{package} v1.0.0|\n")
        for before, after in (("alloc,form,std", "alloc,form,std,parser"),
                              ("alloc,form,std", "alloc,form,std,domain"),
                              ("sqlite-bundled", "sqlite-bundled,pool"),
                              ("sqlite-bundled", "sqlite-bundled,url")):
            with self.assertRaises(AssertionError):
                module.assert_direct_dependencies(direct.replace(before, after))

    def test_required_regressions_must_run_and_pass(self):
        for output in ("", "test unit::required ... ignored", "test unit::required ... FAILED"):
            with patch.object(module.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, output, "")):
                with self.assertRaises(AssertionError):
                    module.execute(["cargo"], ROOT, required_tests=("required",))
        with patch.object(module.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "test unit::required ... ok", "")):
            module.execute(["cargo"], ROOT, required_tests=("required",))

    def test_zero_or_missing_tests_cannot_claim_coverage(self):
        for text in ("", "test result: ok. 0 passed; 0 failed;", "test result: ok. 7 passed; 0 failed;"):
            with patch.object(module.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, text, "")):
                with self.assertRaises(AssertionError):
                    module.execute(["cargo"], ROOT, minimum_tests=8)

    def test_aggregate_preserved_tests_count_is_checked(self):
        output = "test result: ok. 6 passed; 0 failed;\ntest result: ok. 2 passed; 0 failed;\n"
        with patch.object(module.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, output, "")):
            module.execute(["cargo"], ROOT, minimum_tests=8)

    def test_negative_requires_failed_compilation_and_exact_diagnostic(self):
        for status, stderr in ((0, "error[E0603]"), (1, "unrelated failure")):
            with patch.object(module.subprocess, "run", return_value=subprocess.CompletedProcess([], status, "", stderr)):
                with self.assertRaises(AssertionError):
                    module.execute(["cargo"], ROOT, expected="error[E0603]")
        with patch.object(module.subprocess, "run", return_value=subprocess.CompletedProcess([], 1, "", "error[E0603]")):
            module.execute(["cargo"], ROOT, expected="error[E0603]")


if __name__ == "__main__":
    unittest.main()
