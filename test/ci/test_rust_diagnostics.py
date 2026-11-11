"""Fail closed on dependency warnings, malformed Cargo output and failed builds."""
import importlib.util
import copy
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


SCRIPT = (Path(__file__).resolve().parents[2] / ".github/scripts") / "check-rust-diagnostics.py"
SPEC = importlib.util.spec_from_file_location("rust_diagnostics", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def diagnostic(level="warning", rendered="warning: local dependency warning\n"):
    return {"reason": "compiler-message", "package_id": "path+file:///source/foundation/local#1.0.0",
            "message": {"level": level, "message": "local dependency warning", "rendered": rendered}}


def finished(success=True):
    return {"reason": "build-finished", "success": success}


class DiagnosticStreamTests(unittest.TestCase):
    def check(self, events, returncode=0):
        output = io.StringIO()
        checker = MODULE.CargoDiagnostics(output)
        for number, event in enumerate(events, 1):
            checker.consume(json.dumps(event), number)
        return checker.result(returncode), output.getvalue()

    def test_success_with_artifacts_and_native_build_script_metadata(self):
        result, output = self.check([
            {"reason": "build-script-executed", "env": [["BINDGEN", "generated.rs"]]},
            {"reason": "compiler-artifact", "fresh": True}, finished(),
        ])
        self.assertEqual((result, output), (0, ""))

    def test_dependency_warning_rejects_even_when_cargo_succeeds(self):
        result, output = self.check([diagnostic(), finished()])
        self.assertNotEqual(result, 0)
        self.assertIn("warning: local dependency warning", output)
        self.assertIn("1 Rust warning(s)", output)

    def test_errors_reject_even_when_a_broken_producer_claims_success(self):
        for level in ("error", "failure-note", "ice"):
            with self.subTest(level=level):
                self.assertNotEqual(self.check([diagnostic(level), finished()])[0], 0)

    def test_notes_and_help_are_rendered_without_rejecting(self):
        for level in ("note", "help"):
            with self.subTest(level=level):
                result, output = self.check([diagnostic(level, f"{level}: context\n"), finished()])
                self.assertEqual((result, output), (0, f"{level}: context\n"))

    def test_diagnostic_without_rendered_text_remains_visible(self):
        result, output = self.check([diagnostic(rendered=None), finished()])
        self.assertNotEqual(result, 0)
        self.assertIn("warning: local dependency warning", output)

    def test_failed_or_missing_completion_cannot_pass(self):
        for events, code in (([], 0), ([finished(False)], 0), ([finished()], 101)):
            with self.subTest(events=events, code=code):
                self.assertNotEqual(self.check(events, code)[0], 0)

    def test_invalid_or_unknown_messages_cannot_pass(self):
        invalid = [None, [], {}, {"reason": "unexpected"},
                   {"reason": "compiler-message", "message": "not a diagnostic"},
                   diagnostic("unknown"), diagnostic(rendered=[]), finished("true")]
        for event in invalid:
            with self.subTest(event=event):
                events = [event]
                if not isinstance(event, dict) or event.get("reason") != "build-finished":
                    events.append(finished())
                self.assertNotEqual(self.check(events)[0], 0)

    def test_duplicate_completion_and_messages_after_completion_cannot_pass(self):
        for event in (finished(), {"reason": "compiler-artifact"}, diagnostic("note")):
            with self.subTest(event=event):
                self.assertNotEqual(self.check([finished(), event])[0], 0)

    def test_malformed_json_is_not_hidden_by_later_success(self):
        for text in ("", "warning: not Cargo JSON", '{"reason":', '{"reason": NaN}',
                     '{"reason":"compiler-message","reason":"compiler-artifact"}'):
            with self.subTest(text=text):
                output = io.StringIO()
                checker = MODULE.CargoDiagnostics(output)
                checker.consume(text, 1)
                checker.consume(json.dumps(finished()), 2)
                self.assertNotEqual(checker.result(0), 0)
                self.assertIn("invalid Cargo JSON", output.getvalue())


class DiagnosticCommandTests(unittest.TestCase):
    def command(self, stream, code=0):
        producer = ("import sys; sys.stdout.write(sys.argv[1]); "
                    "sys.stderr.write('Cargo progress\\n'); sys.exit(int(sys.argv[2]))")
        return subprocess.run([sys.executable, str(SCRIPT), "--", sys.executable,
                               "-c", producer, stream, str(code)],
                              capture_output=True, text=True, check=False)

    def test_real_child_exit_and_stderr_are_preserved(self):
        result = self.command(json.dumps(finished()) + "\n")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "")
        self.assertIn("Cargo progress", result.stderr)
        self.assertEqual(self.command(json.dumps(finished()) + "\n", 17).returncode, 17)

    def test_warning_stream_rejects_a_successful_child(self):
        result = self.command("\n".join(map(json.dumps, [diagnostic(), finished()])) + "\n")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("1 Rust warning(s)", result.stderr)

    def test_partial_stream_and_invalid_json_cannot_pass(self):
        for stream in ("", "not JSON\n", json.dumps(diagnostic("note")) + "\n"):
            with self.subTest(stream=stream):
                self.assertNotEqual(self.command(stream).returncode, 0)

    def test_missing_command_and_failed_launch_are_errors(self):
        for args in ([], ["--", "/nonexistent/storage-cargo"]):
            with self.subTest(args=args):
                result = subprocess.run([sys.executable, str(SCRIPT), *args],
                                        capture_output=True, text=True, check=False)
                self.assertNotEqual(result.returncode, 0)

    def test_command_arguments_and_compiler_environment_are_preserved(self):
        compiler_environment = {
            "RUSTFLAGS": "--cfg original_product_configuration",
            "CARGO_ENCODED_RUSTFLAGS": "--cfg\x1foriginal_encoded_configuration",
            "CARGO_TARGET_DIR": "/tmp/original-product-cache",
        }
        producer = (
            "import json, os, sys; "
            f"assert sys.argv[1:] == {['--locked', '--offline', '--message-format=json']!r}; "
            f"assert all(os.environ[k] == v for k, v in {compiler_environment!r}.items()); "
            "print(json.dumps({'reason': 'build-finished', 'success': True}))"
        )
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--", sys.executable, "-c", producer,
             "--locked", "--offline", "--message-format=json"],
            capture_output=True, text=True, check=False,
            env=dict(os.environ, **compiler_environment),
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_invalid_utf8_cannot_pass(self):
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--", sys.executable, "-c",
             "import sys; sys.stdout.buffer.write(b'\\xff\\n')"],
            capture_output=True, check=False,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b"not valid UTF-8", result.stderr)

    def test_clippy_options_cannot_enable_warning_exceptions_for_other_commands(self):
        for options in (
            ["--capture-clippy-baseline"],
            ["--clippy-scope", "quality"],
            ["--clippy-report", "observed.json"],
            ["--clippy-scope", "quality", "--clippy-report", "observed.json"],
        ):
            with self.subTest(options=options):
                result = subprocess.run(
                    [sys.executable, str(SCRIPT), *options, "--", sys.executable, "-c", "pass"],
                    capture_output=True, text=True, check=False,
                )
                self.assertEqual(result.returncode, 2)


class ClippyWarningRatchetTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="clippy-warning-fixture-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        (self.root / "rust-toolchain.toml").write_text('[toolchain]\nchannel = "fixture"\n')
        self.metadata = {
            "workspace_root": str(self.root),
            "workspace_members": ["web-id", "smb-id", "foundation-id"],
            "packages": [
                {"id": identity, "name": name,
                 "manifest_path": str(self.root / directory / "Cargo.toml")}
                for identity, name, directory in (
                    ("web-id", "web-app", "src/web"),
                    ("smb-id", "smb-vfs", "src/smb-vfs"),
                    ("foundation-id", "local-dependency", "src/foundation/local"),
                    ("nonmember-id", "not-selected", "external"),
                )
            ],
        }

    def measurement(self, scope="quality", capture=False):
        return MODULE.ClippyWarnings(
            self.root, scope, Path("observed.json"), capture,
            ["cargo", "clippy", "--message-format=json"], self.metadata,
        )

    def event(self, code=MODULE.CLIPPY_WARNINGS[0], identity="web-id", level="warning"):
        event = diagnostic(level)
        event["package_id"] = identity
        event["message"]["code"] = {"code": code}
        return event

    def consume(self, measurement, events, returncode=0):
        output = io.StringIO()
        checker = MODULE.CargoDiagnostics(output, measurement)
        for number, event in enumerate(events, 1):
            checker.consume(json.dumps(event), number)
        return checker.result(returncode), output

    def baseline(self, measurement, value=None):
        # Each fixture baseline records its synthetic observations in the temporary root.
        measurement.baseline.parent.mkdir(parents=True, exist_ok=True)
        measurement.baseline.write_text(json.dumps(measurement.current if value is None else value))

    def test_only_exact_codes_from_metadata_selected_first_party_are_measured(self):
        measurement = self.measurement()
        result, output = self.consume(measurement, [
            *(self.event(code) for code in MODULE.CLIPPY_WARNINGS), finished(),
        ])
        self.assertEqual(result, 0, output.getvalue())
        self.assertEqual(set(measurement.current["counts"]), {"src/web/Cargo.toml"})
        self.assertTrue(all(count == 1 for count in measurement.current["counts"]["src/web/Cargo.toml"].values()))
        self.assertEqual(output.getvalue().count("warning: local dependency warning"), 8)
        self.baseline(measurement)
        self.assertEqual(measurement.finish(result, output), 0)

    def test_same_code_in_foundation_unknown_or_other_scope_is_still_fatal(self):
        for identity in ("foundation-id", "nonmember-id", "smb-id", "unknown-id", None, []):
            with self.subTest(identity=identity):
                measurement = self.measurement()
                result, output = self.consume(measurement, [self.event(identity=identity), finished()])
                self.assertNotEqual(result, 0)
                self.assertIn("1 Rust warning(s)", output.getvalue())
                self.assertFalse(any(measurement.current["counts"]["src/web/Cargo.toml"].values()))

    def test_unknown_missing_or_malformed_codes_remain_fatal(self):
        for code in (None, "unused_variables", "clippy::all", [], {}, {"code": []}):
            with self.subTest(code=code):
                event = self.event()
                event["message"]["code"] = code
                self.assertNotEqual(self.consume(self.measurement(), [event, finished()])[0], 0)

    def test_sdk_inventory_and_diagnostic_package_ids_are_separate(self):
        measurement = self.measurement("smb-sdk")
        self.assertEqual(set(measurement.current["counts"]), {"src/smb-vfs/Cargo.toml"})
        self.assertEqual(self.consume(measurement, [self.event(identity="smb-id"), finished()])[0], 0)
        self.assertNotEqual(self.consume(measurement, [self.event(identity="web-id"), finished()])[0], 0)

    def test_new_codes_are_fatal_without_explicit_opt_in_and_errors_are_never_counted(self):
        for level in ("warning", "error", "failure-note", "ice"):
            with self.subTest(level=level):
                checker = MODULE.CargoDiagnostics(io.StringIO())
                checker.consume(json.dumps(self.event(level=level)), 1)
                checker.consume(json.dumps(finished()), 2)
                self.assertNotEqual(checker.result(0), 0)
                if level != "warning":
                    measurement = self.measurement()
                    self.assertNotEqual(self.consume(measurement, [self.event(level=level), finished()])[0], 0)
                    self.assertFalse(any(measurement.current["counts"]["src/web/Cargo.toml"].values()))

    def test_missing_baseline_fails_after_writing_complete_observed_counts(self):
        measurement = self.measurement()
        result, output = self.consume(measurement, [self.event(), self.event(), finished()])
        self.assertNotEqual(measurement.finish(result, output), 0)
        report = json.loads(measurement.report.read_text())
        self.assertIs(report["complete"], True)
        self.assertEqual(report["baseline_candidate"]["counts"]["src/web/Cargo.toml"][MODULE.CLIPPY_WARNINGS[0]], 2)
        self.assertFalse(measurement.baseline.exists())
        self.assertIn("reviewed baseline unavailable", output.getvalue())

    def test_explicit_capture_never_passes_or_overwrites_an_existing_baseline(self):
        measurement = self.measurement(capture=True)
        self.baseline(measurement)
        original = measurement.baseline.read_bytes()
        result, output = self.consume(measurement, [finished()])
        self.assertNotEqual(measurement.finish(result, output), 0)
        self.assertEqual(measurement.baseline.read_bytes(), original)
        self.assertIn("capture is not a passing ratchet", output.getvalue())

    def test_rises_and_falls_both_require_a_reviewed_baseline_update(self):
        for count, expected in ((0, "rose"), (2, "fell")):
            with self.subTest(count=count):
                measurement = self.measurement()
                baseline = copy.deepcopy(measurement.current)
                baseline["counts"]["src/web/Cargo.toml"][MODULE.CLIPPY_WARNINGS[0]] = count
                self.baseline(measurement, baseline)
                original = measurement.baseline.read_bytes()
                result, output = self.consume(measurement, [self.event(), finished()])
                self.assertNotEqual(measurement.finish(result, output), 0)
                self.assertIn(f"[clippy-warnings] {expected}", output.getvalue())
                self.assertEqual(measurement.baseline.read_bytes(), original)

    def test_failed_or_invalid_stream_cannot_produce_a_complete_candidate(self):
        cases = [([self.event()], 0), ([self.event(), finished(False)], 0),
                 ([self.event(), finished()], 101), ([self.event(), diagnostic(), finished()], 0),
                 ([self.event(), {"reason": "unexpected"}, finished()], 0),
                 ([self.event(), finished(), finished()], 0)]
        for events, returncode in cases:
            with self.subTest(events=events, returncode=returncode):
                measurement = self.measurement()
                measurement.write_report(True)
                measurement.write_report(False)
                self.assertIs(json.loads(measurement.report.read_text())["complete"], False)
                result, output = self.consume(measurement, events, returncode)
                self.assertNotEqual(measurement.finish(result, output), 0)
                self.assertIs(json.loads(measurement.report.read_text())["complete"], False)

    def test_baseline_schema_inventory_and_count_types_are_exact(self):
        measurement = self.measurement()
        invalid = [None, [], {}, dict(measurement.current, schema_version=True),
                   dict(measurement.current, scope="smb-sdk"),
                   dict(measurement.current, toolchain="other"),
                   dict(measurement.current, command=[]),
                   dict(measurement.current, counts={})]
        for count in (True, -1, 1.5, "0", None):
            baseline = copy.deepcopy(measurement.current)
            baseline["counts"]["src/web/Cargo.toml"][MODULE.CLIPPY_WARNINGS[0]] = count
            invalid.append(baseline)
        for code in ("clippy::unknown", MODULE.CLIPPY_WARNINGS[0]):
            baseline = copy.deepcopy(measurement.current)
            baseline["counts"]["src/web/Cargo.toml"].pop(MODULE.CLIPPY_WARNINGS[0])
            if code == "clippy::unknown":
                baseline["counts"]["src/web/Cargo.toml"][code] = 0
            invalid.append(baseline)
        for baseline in invalid:
            with self.subTest(baseline=baseline):
                with self.assertRaises(ValueError):
                    measurement.validate_baseline(baseline)

    def test_duplicate_baseline_keys_are_rejected_without_accepting_observation(self):
        measurement = self.measurement()
        self.baseline(measurement)
        measurement.baseline.write_text('{"schema_version":1,"schema_version":1}')
        result, output = self.consume(measurement, [finished()])
        self.assertNotEqual(measurement.finish(result, output), 0)
        self.assertIn("duplicate JSON key", output.getvalue())

    def test_report_cannot_replace_reviewed_baseline(self):
        with self.assertRaisesRegex(ValueError, "must not overwrite"):
            MODULE.ClippyWarnings(
                self.root, "quality", Path(".github/clippy-warnings-baseline/quality.json"), False,
                ["cargo", "clippy", "--message-format=json"], self.metadata,
            )


if __name__ == "__main__":
    unittest.main()
