"""Native build receipt tests use fake subprocesses, never a Rust compiler."""
import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("release_native", ROOT / ".github/scripts/release-native.py")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
REAL_CHECK_OUTPUT = subprocess.check_output


class NativeRelease(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        for name in ("Cargo.lock", "Cargo.toml", ".cargo/config.toml", "rust-toolchain.toml"):
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("fixture input")
        web_manifest = self.root / "build/web/build-manifest.json"
        web_manifest.parent.mkdir(parents=True)
        web_manifest.write_text("fixture Web manifest")
        self.enterContext(patch.object(MODULE, "require_debian13"))
        self.enterContext(patch.object(MODULE, "runtime_dependencies", return_value={"fixture": "not runtime evidence"}))
        self.output = self.root / MODULE.OUTPUT
        self.output.mkdir(parents=True)
        for name in MODULE.BINARIES:
            (self.output / name).write_bytes(name.encode())
        self.receipt = self.output / "BUILD.txt"
        self.receipt.write_text("stale successful build")
        self.enterContext(patch.object(MODULE, "__file__", str(self.root / ".github/scripts/release-native.py")))
        self.enterContext(patch.object(MODULE.os, "chdir"))
        self.enterContext(patch.dict(os.environ, {"RELEASE_VERSION": "0.1.0", "CFLAGS": "-DPRIVATE=secret"}, clear=True))
        self.commit = "a" * 40
        self.source_status = ""
        self.queries = self.enterContext(patch.object(MODULE.subprocess, "check_output", side_effect=self.query))

    def query(self, command, **kwargs):
        if command == ["git", "rev-parse", "HEAD"]:
            return self.commit + "\n"
        if command == ["git", "rev-parse", "HEAD^{tree}"]:
            return "b" * 40 + "\n"
        if command == ["git", "status", "--porcelain", "--untracked-files=normal"]:
            return self.source_status
        if "target-cpus" in command:
            return "    x86-64 - This is the default target CPU for the current build target.\n"
        if "cfg" in command:
            return 'debug_assertions\ntarget_arch="x86_64"\ntarget_feature="sse2"\ntarget_os="linux"\n'
        if command[-1] == "-vV":
            return "rustc fixture\nhost: x86_64-unknown-linux-gnu\n"
        self.fail(f"unexpected native build query: {command}")

    def test_build_uses_explicit_target_and_records_outputs_without_environment_values(self):
        with patch.object(MODULE.subprocess, "run") as run:
            MODULE.main()
        command = run.call_args.args[0]
        self.assertEqual(command[command.index("--target") + 1], MODULE.TARGET)
        self.assertEqual(command[command.index("--target-dir") + 1], "target/native")
        self.assertIn("storage-server", command)
        self.assertIn("storage-rescue", command)
        self.assertEqual(run.call_args.kwargs["env"]["ZXS_EMBED_WEB"], "1")
        self.assertEqual(run.call_args.kwargs["env"]["ZXS_EMBED_WEB_COMMIT"], self.commit)
        self.assertEqual(run.call_args.kwargs["env"]["ZXS_EMBED_WEB_TREE"], "b" * 40)
        content = self.receipt.read_text()
        self.assertIn("native-target=x86_64-unknown-linux-gnu\n", content)
        self.assertIn("native-zxs-sha256=" + MODULE.digest(self.output / "zxs"), content)
        self.assertIn("native-zxs-rescue-sha256=" + MODULE.digest(self.output / "zxs-rescue"), content)
        self.assertNotIn("secret", content)
        self.assertNotIn("stale", content)
        self.assertIn("native-rust-target-default-cpu=x86-64\n", content)
        self.assertIn('native-rust-flag-overrides-present=false\n', content)
        self.assertIn('native-c-flag-overrides-present=true\n', content)
        self.assertIn('native-isa-attestation=not-attested;', content)
        self.assertNotIn('debug_assertions', content)

    def test_web_manifest_change_during_build_rejects_receipt(self):
        with patch.object(MODULE.subprocess, "run", side_effect=lambda *args, **kwargs:
                          (self.root / "build/web/build-manifest.json").write_text("different Web")):
            with self.assertRaisesRegex(RuntimeError, "embedded Web manifest changed"):
                MODULE.main()
        self.assertFalse(self.receipt.exists())

    def test_failed_build_removes_old_receipt(self):
        with patch.object(MODULE.subprocess, "run", side_effect=subprocess.CalledProcessError(1, "cargo")):
            with self.assertRaises(subprocess.CalledProcessError):
                MODULE.main()
        self.assertFalse(self.receipt.exists())

    def test_cargo_build_rustc_override_is_used_for_compiler_query(self):
        with patch.dict(os.environ, {"CARGO_BUILD_RUSTC": "/private/toolchain/rustc"}), \
                patch.object(MODULE.subprocess, "run"):
            MODULE.main()
        self.assertIn(["/private/toolchain/rustc", "-vV"],
                      [call.args[0] for call in MODULE.subprocess.check_output.call_args_list])
        self.assertNotIn("/private/toolchain", self.receipt.read_text())

    def test_dirty_source_never_starts_a_release_build_or_keeps_old_receipt(self):
        for status in (" M src/server/src/lib.rs\n", "M  src/server/src/lib.rs\n",
                       "?? src/server/src/new_module.rs\n"):
            with self.subTest(status=status):
                self.receipt.write_text("stale successful build")
                self.source_status = status
                with patch.object(MODULE.subprocess, "run") as run:
                    with self.assertRaisesRegex(RuntimeError, "clean source checkout"):
                        MODULE.main()
                run.assert_not_called()
                self.assertFalse(self.receipt.exists())

    def test_same_commit_cannot_hide_source_changes_during_the_build(self):
        with patch.object(MODULE.subprocess, "run", side_effect=lambda *args, **kwargs:
                          setattr(self, "source_status", " M src/server/src/lib.rs\n")) as run:
            with self.assertRaisesRegex(RuntimeError, "clean source checkout"):
                MODULE.main()
        run.assert_called_once()
        self.assertFalse(self.receipt.exists())

    def test_source_commit_changes_do_not_leave_a_receipt(self):
        with patch.object(MODULE.subprocess, "run", side_effect=lambda *args, **kwargs:
                          setattr(self, "commit", "b" * 40)):
            with self.assertRaisesRegex(RuntimeError, "source commit changed"):
                MODULE.main()
        self.assertFalse(self.receipt.exists())

    def test_build_input_changes_do_not_leave_a_receipt(self):
        with patch.object(MODULE.subprocess, "run", side_effect=lambda *args, **kwargs:
                          (self.root / ".cargo/config.toml").write_text("changed configuration")):
            with self.assertRaisesRegex(RuntimeError, "configuration changed"):
                MODULE.main()
        self.assertFalse(self.receipt.exists())

    def real_source(self, build):
        source = self.root / "src/main.rs"
        source.parent.mkdir()
        source.write_text("original source")
        (self.root / ".gitignore").write_text("target/\n")
        subprocess.run(["git", "init", "--quiet", str(self.root)], check=True)
        subprocess.run(["git", "-C", str(self.root), "add", "."], check=True)
        subprocess.run(["git", "-C", str(self.root), "-c", "user.name=Fixture",
                        "-c", "user.email=fixture@example.invalid", "commit", "--quiet", "-m", "fixture"], check=True)
        def query(command, **kwargs):
            if command[0] == "git":
                return REAL_CHECK_OUTPUT(command, cwd=self.root, **kwargs)
            return self.query(command, **kwargs)

        return source, SimpleNamespace(check_output=query, run=build)

    def test_real_git_clean_checkout_allows_ignored_build_outputs(self):
        _, process = self.real_source(lambda *args, **kwargs: None)
        with patch.object(MODULE, "subprocess", process):
            MODULE.main()
        commit = REAL_CHECK_OUTPUT(["git", "rev-parse", "HEAD"], cwd=self.root, text=True).strip()
        self.assertIn(f"commit={commit}\n", self.receipt.read_text())

    def test_real_git_rejects_uncommitted_source_before_build(self):
        build = unittest.mock.Mock()
        source, process = self.real_source(build)
        source.write_text("modified source")
        with patch.object(MODULE, "subprocess", process):
            with self.assertRaisesRegex(RuntimeError, "clean source checkout"):
                MODULE.main()
        build.assert_not_called()
        self.assertFalse(self.receipt.exists())

    def test_real_git_rejects_source_change_during_build_at_the_same_commit(self):
        source, process = self.real_source(lambda *args, **kwargs: source.write_text("changed during build"))
        with patch.object(MODULE, "subprocess", process):
            with self.assertRaisesRegex(RuntimeError, "clean source checkout"):
                MODULE.main()
        self.assertFalse(self.receipt.exists())




class NativeTargetDefaults(unittest.TestCase):
    def test_actual_flags_are_not_claimed_and_override_values_remain_private(self):
        values = {"CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS": "-Ctarget-cpu=secret-host",
                  "CFLAGS_x86_64_unknown_linux_gnu": "-DPRIVATE=secret", "CC": "/private/compiler"}
        result = MODULE.override_presence(values)
        self.assertTrue(result['native-rust-flag-overrides-present'])
        self.assertTrue(result['native-c-flag-overrides-present'])
        self.assertTrue(result['native-build-overrides-present'])
        self.assertTrue(all(type(value) is bool for value in result.values()))
        self.assertNotIn('secret', str(result))
        self.assertNotIn('/private', str(result))
        self.assertFalse(any(MODULE.override_presence({}).values()))

    def test_probe_rejects_missing_ambiguous_or_private_cpu_without_echoing_values(self):
        for output in ["no default", " /private/cpu - default target CPU", " x86-64 - default target CPU\n generic - default target CPU"]:
            with patch.object(MODULE.subprocess, 'check_output', return_value=output):
                with self.assertRaises(RuntimeError) as failure:
                    MODULE.target_defaults('rustc')
                self.assertNotIn('/private', str(failure.exception))

    def test_probe_filters_nontarget_cfg_and_rejects_unsafe_or_wrong_target_values(self):
        cpu=' x86-64 - default target CPU'
        for cfg in ['target_arch="aarch64"', 'target_arch="x86_64"\ntarget_feature="private/path"']:
            with patch.object(MODULE.subprocess,'check_output',side_effect=[cpu,cfg]):
                with self.assertRaises(RuntimeError) as failure:
                    MODULE.target_defaults('rustc')
                self.assertNotIn('private/path',str(failure.exception))
        with patch.object(MODULE.subprocess,'check_output',side_effect=[cpu,'private_unrelated_cfg="secret"\ntarget_arch="x86_64"\ntarget_feature="sse2"']) as query:
            actual=MODULE.target_defaults('rustc')
            self.assertEqual(actual,('x86-64',['target_arch="x86_64"','target_feature="sse2"']))
            self.assertTrue(all(call.args[0][-2:]==['--target',MODULE.TARGET] for call in query.call_args_list))


if __name__ == "__main__":
    unittest.main()
