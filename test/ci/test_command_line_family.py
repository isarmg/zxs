import importlib.util
import json
import os
import subprocess
import tempfile
import unittest
from unittest import mock
from pathlib import Path
import runpy

ROOT = Path(__file__).resolve().parents[2]
foundation_path = runpy.run_path(str(ROOT / '.github/scripts/foundation_paths.py'))['package_path']
spec = importlib.util.spec_from_file_location('gate', ROOT / '.github/scripts/check-command-line-family.py')
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)

class NamingGate(unittest.TestCase):

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        packages = {}
        for old, new in gate.N.items():
            p = self.root / 'src/foundation/tooling' / new
            (p / 'src').mkdir(parents=True)
            (p / 'Cargo.toml').write_text(f'''[package]\nname="{new}"\n[lib]\nname="{new.replace('-', '_')}"\n''')
            (p / 'src/lib.rs').write_text('attributes(command, arg, group)' if new == 'command-derive' else 'mod lexer;')
            packages['src/foundation/tooling/' + new] = {'directory': 'src/foundation/tooling/' + new, 'origin_directory': old + '-4.6.7'}
        suggestions = self.root / 'src/foundation/tooling/command-line/src/parser/features/suggestions/mod.rs'
        suggestions.parent.mkdir(parents=True)
        suggestions.write_text('mod similarity;\nfn compare(v: &str, pv: &String) -> f64 { similarity::jaro(v, pv.as_ref()) }\n')
        for component in ('src/server', 'helper'):
            p = self.root / component / 'src/main.rs'
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text('use command_line::Parser;')
        (self.root / 'compliance').mkdir()
        (self.root / 'compliance/rust-sources.json').write_text(json.dumps({'packages': packages, 'merges': [{'directory': 'src/foundation/tooling/command-line', 'origin_directory': origin} for origin in ('clap_builder-4.6.7', 'clap_lex-1.1.1')]}))

    def test_accepts_functional_identity(self):
        self.assertEqual(gate.check(self.root), 2)

    def test_rejects_missing_private_similarity_module(self):
        path = self.root / 'src/foundation/tooling/command-line/src/parser/features/suggestions/mod.rs'
        path.write_text(path.read_text().replace('mod similarity;', 'mod other;'))
        with self.assertRaisesRegex(AssertionError, 'missing private similarity module'):
            gate.check(self.root)

    def test_rejects_public_similarity_module(self):
        path = self.root / 'src/foundation/tooling/command-line/src/parser/features/suggestions/mod.rs'
        original = path.read_text()
        for visibility in ('pub', 'pub(crate)'):
            with self.subTest(visibility=visibility):
                path.write_text(original.replace('mod similarity;', visibility + ' mod similarity;'))
                with self.assertRaisesRegex(AssertionError, 'similarity module became public'):
                    gate.check(self.root)

    def test_rejects_bypassed_moved_similarity_implementation(self):
        path = self.root / 'src/foundation/tooling/command-line/src/parser/features/suggestions/mod.rs'
        path.write_text(path.read_text().replace('similarity::jaro(v, pv.as_ref())', '0.0'))
        with self.assertRaisesRegex(AssertionError, 'suggestions bypass moved Jaro implementation'):
            gate.check(self.root)

    def test_rejects_removed_capabilities(self):
        for name in ('unicode', 'wrap_help', 'unstable-markdown'):
            with self.subTest(feature=name):
                path = self.root / 'src/foundation/tooling/command-line/Cargo.toml'
                before = path.read_text()
                path.write_text(before + '[features]\n' + name + '=[]\n')
                with self.assertRaisesRegex(AssertionError, 'unsupported capability reintroduced'):
                    gate.check(self.root)
                path.write_text(before)

    def test_rejects_unmerged_package(self):
        (self.root / 'src/foundation/command-builder').mkdir()
        with self.assertRaisesRegex(AssertionError, 'unmerged source root'):
            gate.check(self.root)

    def test_rejects_public_lexer(self):
        (self.root / 'src/foundation/tooling/command-line/src/lib.rs').write_text('pub mod lexer;')
        with self.assertRaisesRegex(AssertionError, 'lexer exposed'):
            gate.check(self.root)

    def test_rejects_lost_merged_origin(self):
        path = self.root / 'compliance/rust-sources.json'
        catalog = json.loads(path.read_text())
        catalog['merges'] = []
        path.write_text(json.dumps(catalog))
        with self.assertRaisesRegex(AssertionError, 'lost merged runtime upstream origin'):
            gate.check(self.root)

    def test_rejects_legacy_root(self):
        (self.root / 'src/foundation/clap').mkdir()
        with self.assertRaisesRegex(AssertionError, 'old source root'):
            gate.check(self.root)

    def test_rejects_legacy_emitted_path(self):
        (self.root / 'src/foundation/tooling/command-derive/src/lib.rs').write_text('attributes(command, arg, group) impl clap::Parser for X {}')
        with self.assertRaisesRegex(AssertionError, 'old Rust path'):
            gate.check(self.root)

    def test_rejects_legacy_attribute(self):
        (self.root / 'src/foundation/tooling/command-derive/src/lib.rs').write_text('attributes(command, arg, group) attributes(clap)')
        with self.assertRaisesRegex(AssertionError, 'old derive alias'):
            gate.check(self.root)

    def test_rejects_dependency_alias(self):
        p = self.root / 'src/foundation/tooling/command-line/Cargo.toml'
        p.write_text(p.read_text() + '[dependencies.clap]\npackage="command-line"\npath="../command-line"\n')
        with self.assertRaisesRegex(AssertionError, 'old dependency alias'):
            gate.check(self.root)

    def test_cargo_commands_are_locked_and_offline(self):
        for operation in ('test', 'check', 'build', 'metadata'):
            self.assertEqual(gate.cargo_command(operation, '--manifest-path', 'fixture/Cargo.toml'), ['cargo', operation, '--locked', '--offline', '--color', 'never', '--manifest-path', 'fixture/Cargo.toml'])

    def test_captured_cargo_diagnostics_override_forced_ci_color(self):
        with mock.patch.dict(os.environ, {"CARGO_TERM_COLOR": "always"}):
            command = gate.cargo_command("check", "--manifest-path", "fixture/Cargo.toml")
            self.assertEqual(command[command.index("--color") + 1], "never")
            self.assertEqual(os.environ["CARGO_TERM_COLOR"], "always")

    def test_unsafe_rejection_accepts_actual_colored_compiler_content(self):
        stderr = "\x1b[1m\x1b[91merror\x1b[0m\x1b[1m: usage of an `unsafe` block\x1b[0m\nrejected_unsafe_boundary_probe"
        gate.require_unsafe_rejection(subprocess.CompletedProcess([], 101, "", stderr), "src/lib.rs")

    def test_unsafe_rejection_requires_error_and_injected_code(self):
        expected = "error: usage of an `unsafe` block\nrejected_unsafe_boundary_probe"
        for status, stderr in ((0, expected), (101, "unrelated failure"),
                               (101, "error: usage of an `unsafe` block"),
                               (101, "warning: usage of an `unsafe` block\nrejected_unsafe_boundary_probe")):
            with self.subTest(status=status, stderr=stderr), self.assertRaises(AssertionError):
                gate.require_unsafe_rejection(subprocess.CompletedProcess([], status, "", stderr), "src/lib.rs")

    def check_consumer_result(self, result):
        fixture = self.root / 'test/fixtures/command-line-cli.json'
        fixture.parent.mkdir(parents=True, exist_ok=True)
        fixture.write_text(json.dumps([{
            "binary": "web-build", "args": ["check", "--invalid"],
            "code": 1, "stdout": "", "stderr": "Error: unknown option --invalid\n",
        }]))
        metadata = json.dumps({"target_directory": str(self.root / 'target')})
        with mock.patch.object(gate.subprocess, 'check_output', return_value=metadata), \
                mock.patch.object(gate.subprocess, 'run', return_value=result) as run:
            gate.check_consumers(self.root)
        return run.call_args_list

    def test_consumer_goldens_disable_only_local_optional_backtraces(self):
        polluted = {
            "RUST_BACKTRACE": "1", "RUST_LIB_BACKTRACE": "full",
            "CARGO_TERM_COLOR": "always", "PATH": os.environ.get("PATH", ""),
        }
        result = subprocess.CompletedProcess([], 1, "", "Error: unknown option --invalid\n")
        with mock.patch.dict(os.environ, polluted):
            original = dict(os.environ)
            calls = self.check_consumer_result(result)
            self.assertEqual(dict(os.environ), original)
        self.assertNotIn('env', calls[0].kwargs)
        self.assertNotIn('env', calls[1].kwargs)
        self.assertEqual(calls[2].kwargs['env'], {
            **original, "RUST_BACKTRACE": "0", "RUST_LIB_BACKTRACE": "0",
        })
        self.assertTrue(calls[2].kwargs['capture_output'])

    def test_consumer_goldens_reject_any_output_or_status_drift(self):
        stderr = "Error: unknown option --invalid\n"
        for status, stdout, error in (
            (0, "", stderr), (1, "unexpected output", stderr),
            (1, "", stderr + "unexpected diagnostic\n"),
            (1, "", "\x1b[31m" + stderr + "\x1b[0m"),
            (1, "", stderr + "\nStack backtrace:\n unexpected trace\n"),
        ):
            with self.subTest(status=status, stdout=stdout, stderr=error), \
                    self.assertRaisesRegex(AssertionError, 'CLI contract drift'):
                self.check_consumer_result(subprocess.CompletedProcess([], status, stdout, error))

    def test_only_temporary_fixture_generates_a_lock(self):
        fixture = self.root / 'test/command-line-family.rs'
        fixture.parent.mkdir()
        fixture.write_text('// fixture')
        (fixture.parent / 'command-line-lexer-examples.rs').write_text('// lexer examples')
        with tempfile.TemporaryDirectory() as directory:
            with mock.patch.object(gate.subprocess, 'run') as run:
                manifest, source = gate.prepare_fixture(self.root, Path(directory))
            run.assert_called_once_with(['cargo', 'generate-lockfile', '--offline', '--manifest-path', str(manifest)], cwd=self.root, check=True)
            self.assertIn('// fixture', source.read_text())
            self.assertIn('mod lexer;', source.read_text())
            self.assertFalse((self.root / 'Cargo.lock').exists())

    def test_rejects_erased_origin(self):
        p = self.root / 'compliance/rust-sources.json'
        d = json.loads(p.read_text())
        d['packages']['src/foundation/tooling/command-line']['origin_directory'] = 'command-line'
        p.write_text(json.dumps(d))
        with self.assertRaisesRegex(AssertionError, 'lost upstream origin'):
            gate.check(self.root)
if __name__ == '__main__':
    unittest.main()
