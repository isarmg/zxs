"""Watch reports, snapshot provenance and independent blocking gate behavior."""
import copy
from contextlib import redirect_stdout, redirect_stderr
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    'boundary_metrics', ROOT / '.github/scripts/check-boundary-metrics.py')
METRICS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(METRICS)


class BoundaryMetricsTests(unittest.TestCase):
    def test_counts_all_declared_sources_without_reading_root_tests(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for source in METRICS.SOURCES:
                (root / source).mkdir(parents=True)
            production = root / METRICS.SOURCES[0] / 'example.rs'
            production.write_text('Error::new(code, "detail");\n', encoding='utf-8')
            tests = root / 'test'
            tests.mkdir()
            (tests / 'example.rs').write_text('validate_id(value);\n', encoding='utf-8')
            result = METRICS.counts(root)
            self.assertEqual(result[METRICS.SOURCES[0]]['error_new'], 1)
            self.assertEqual(result[METRICS.SOURCES[0]]['validate_fn'], 0)
            self.assertEqual(set(result), set(METRICS.SOURCES))

    def test_counts_revision_patterns_and_excludes_comments_and_foundation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for source in METRICS.SOURCES:
                (root / source).mkdir(parents=True)
            production = root / METRICS.SOURCES[0] / 'example.rs'
            production.write_text(
                '// Error::new(code, "comment");\n'
                '    /// validate_id(comment);\n'
                'validate_path(path);\n'
                'file_ids: Vec<String>,\n'
                'file_hash: Option<String>,\n'
                'BTreeMap<String, Item>\n'
                'json_codec::Value\n'
                'ErrorCode::RecoveryRequired\n'
                '_ => unreachable!(),\n'
                'value.map_err(|_| replacement)\n'
                'root.join("metadata/pool.json");\n'
                'strings.join("/");\n'
                'const JOURNAL: &str = "journal.bin";\n'
                'fn require_source() {}\n'
                'engine.state().sequence\n'
                '"/api/files"\n'
                'now()?;\n'
                '#[cfg(feature = "fault-injection")]\n'
                'use super::*;\n'
                '#[cfg(test)]\n#[path = "fixture.rs"]\nmod tests;\n'
                '#[cfg(test)]\nmod inline { }\n'
                '#[cfg(test)]\n', encoding='utf-8')
            excluded = root / 'src/foundation'
            excluded.mkdir()
            (excluded / 'example.rs').write_text('Error::new(code, "detail");\n')
            counted = METRICS.counts(root)[METRICS.SOURCES[0]]
            for metric in (
                'validate_fn', 'id_string', 'hash_string', 'string_key_map',
                'json_value', 'invariant_as_error', 'wildcard_arm', 'unreachable',
                'erase_error', 'join_literal', 'disk_name_const', 'assert_fn_def',
                'state_leak', 'api_path_literal', 'clock_read', 'fault_cfg', 'glob_super',
            ):
                with self.subTest(metric=metric):
                    self.assertEqual(counted[metric], 1)
            self.assertEqual(counted['error_new'], 0)
            self.assertEqual(counted['cfg_test_item'], 2)
            self.assertEqual(counted['long_line'], 0)
            production.write_text('x' * 101 + '\n', encoding='utf-8')
            self.assertEqual(METRICS.counts(root)[METRICS.SOURCES[0]]['long_line'], 1)

    def fixture(self):
        temporary = tempfile.TemporaryDirectory(prefix='boundary-watch-')
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        for source in METRICS.SOURCES:
            (root / source).mkdir(parents=True)
            (root / source / '.keep').touch()
        (root / '.github').mkdir()
        return root

    def baseline(self, root):
        value = {
            'version': 1, 'mode': 'watch', 'commit': 'a' * 40,
            'script_sha256': METRICS.script_digest(),
            'captured_at': '2026-11-11T00:00:00+00:00',
            'scope': list(METRICS.SOURCES), 'patterns': dict(METRICS.PATTERNS),
            'sampling': dict(METRICS.SAMPLING), 'counts': METRICS.counts(root),
        }
        (root / METRICS.BASELINE).write_text(json.dumps(value), encoding='utf-8')
        return value

    def run_watch(self, root, *args):
        stdout, stderr = io.StringIO(), io.StringIO()
        with redirect_stdout(stdout), redirect_stderr(stderr):
            code = METRICS.main(['--root', str(root), *args])
        return code, stdout.getvalue(), stderr.getvalue()

    def test_equal_rising_and_falling_counts_all_pass(self):
        root = self.fixture()
        production = root / METRICS.SOURCES[0] / 'example.rs'
        production.write_text('Error::new(code, "detail");\n', encoding='utf-8')
        self.baseline(root)
        code, output, error = self.run_watch(root)
        self.assertEqual((code, error), (0, ''))
        self.assertIn('watch only:', output)
        for text, direction in [('', 'fell'), ('Error::new(c, "a");\n' * 2, 'rose')]:
            with self.subTest(direction=direction):
                production.write_text(text, encoding='utf-8')
                code, output, error = self.run_watch(root)
                self.assertEqual((code, error), (0, ''))
                self.assertIn(f'{direction} src/engine:error_new:', output)

    def test_legal_matches_remain_visible_without_claiming_violations(self):
        root = self.fixture()
        self.baseline(root)
        (root / METRICS.SOURCES[0] / 'legal.rs').write_text(
            'let parsed = Value::parse(input)?;\n'
            'let joined = strings.join(".");\n'
            'let encoded = json!({"code": status});\n'
            'let bounded = bytes.checked_add(1);\n'
            'match text { "known" => 1, _ => 0 }\n'
            'let impossible = || unreachable!();\n'
            + 'x' * 101 + '\n', encoding='utf-8')
        code, output, error = self.run_watch(root)
        self.assertEqual((code, error), (0, ''))
        for metric in ('parse_discard', 'join_literal', 'json_value', 'checked_add_1',
                       'wildcard_arm', 'unreachable', 'long_line'):
            self.assertIn(f'rose src/engine:{metric}: 0 -> 1', output)

    def test_missing_malformed_and_unversioned_baselines_fail_as_input_errors(self):
        root = self.fixture()
        path = root / METRICS.BASELINE
        for raw in (None, '{', '[]', json.dumps(METRICS.counts(root)),
                    '{"version": 1, "version": 1}'):
            with self.subTest(raw=raw):
                if raw is not None:
                    path.write_text(raw, encoding='utf-8')
                code, output, error = self.run_watch(root)
                self.assertEqual(code, 2)
                self.assertIn('input error:', error)
                self.assertNotIn('watch only:', output)
                self.assertNotIn('Traceback', error)

    def test_damaged_metadata_and_counts_cannot_look_like_zero_debt(self):
        root = self.fixture()
        valid = self.baseline(root)
        mutations = [
            ('commit', 'main'), ('script_sha256', 'missing'), ('captured_at', 'yesterday'),
            ('captured_at', '2026-11-11T00:00:00'), ('scope', []), ('patterns', {}),
            ('sampling', {}), ('counts', {}), ('version', True), ('mode', 'blocking'),
            ('migration', {}),
        ]
        for field, changed in mutations:
            with self.subTest(field=field, changed=changed):
                value = copy.deepcopy(valid)
                value[field] = changed
                (root / METRICS.BASELINE).write_text(json.dumps(value), encoding='utf-8')
                self.assertEqual(self.run_watch(root)[0], 2)
        for changed in (-1, True, '0', None):
            with self.subTest(count=changed):
                value = copy.deepcopy(valid)
                value['counts'][METRICS.SOURCES[0]]['error_new'] = changed
                (root / METRICS.BASELINE).write_text(json.dumps(value), encoding='utf-8')
                self.assertEqual(self.run_watch(root)[0], 2)

    def test_changed_script_digest_is_distinct_from_count_changes(self):
        root = self.fixture()
        value = self.baseline(root)
        value['script_sha256'] = 'b' * 64
        (root / METRICS.BASELINE).write_text(json.dumps(value), encoding='utf-8')
        code, output, error = self.run_watch(root)
        self.assertEqual((code, error), (0, ''))
        self.assertIn('script digest changed', output)
        self.assertNotIn('rose ', output)
        self.assertNotIn('fell ', output)

    def test_inventory_changes_require_explicit_resampling(self):
        baseline = METRICS.empty_counts()
        for edit in ('source', 'metric'):
            current = copy.deepcopy(baseline)
            if edit == 'source':
                del current[METRICS.SOURCES[0]]
            else:
                del current[METRICS.SOURCES[0]]['error_new']
            with self.subTest(edit=edit), self.assertRaises(ValueError):
                METRICS.changes(current, baseline)

    def test_missing_source_is_an_input_error(self):
        root = self.fixture()
        self.baseline(root)
        shutil.rmtree(root / METRICS.SOURCES[0])
        self.assertEqual(self.run_watch(root)[0], 2)

    def commit_fixture(self, root):
        subprocess.run(['git', 'init', '-q', str(root)], check=True, capture_output=True)
        subprocess.run(['git', '-C', str(root), 'add', '.'], check=True, capture_output=True)
        subprocess.run(['git', '-C', str(root), '-c', 'user.name=Fixture',
                        '-c', 'user.email=fixture@example.invalid', '-c', 'commit.gpgsign=false',
                        'commit', '-qm', 'fixture'], check=True, capture_output=True)
        return METRICS.git(root, 'rev-parse', 'HEAD').decode().strip()

    def test_explicit_migration_resamples_the_exact_commit_not_dirty_sources(self):
        root = self.fixture()
        production = root / METRICS.SOURCES[0] / 'example.rs'
        production.write_text('Error::new(code, "detail");\n', encoding='utf-8')
        old = METRICS.counts(root)
        old[METRICS.SOURCES[0]]['error_new'] = 99
        path = root / METRICS.BASELINE
        raw = json.dumps(old).encode()
        path.write_bytes(raw)
        commit = self.commit_fixture(root)
        production.write_text('Error::new(code, "dirty");\n' * 3, encoding='utf-8')
        (production.parent / 'untracked.rs').write_text('Error::new(c, "untracked");\n')
        self.assertEqual(self.run_watch(root, '--update')[0], 2)
        self.assertEqual(path.read_bytes(), raw)
        code, output, error = self.run_watch(root, '--update', '--migrate', '--ref', commit)
        self.assertEqual((code, error), (0, ''))
        value = METRICS.validate_snapshot(json.loads(path.read_text()))
        self.assertEqual(value['commit'], commit)
        self.assertEqual(value['script_sha256'], METRICS.script_digest())
        self.assertEqual(value['counts'][METRICS.SOURCES[0]]['error_new'], 1)
        self.assertEqual(value['migration']['previous_baseline_sha256'],
                         hashlib.sha256(raw).hexdigest())
        self.assertEqual(value['migration']['method'], 'resampled-from-commit')
        self.assertIn('uncommitted source edits are excluded', output)
        code, output, error = self.run_watch(root)
        self.assertEqual((code, error), (0, ''))
        self.assertIn('rose src/engine:error_new: 1 -> 4', output)
        self.assertEqual(self.run_watch(root, '--update', '--migrate')[0], 2)
        self.assertEqual(json.loads(path.read_text()), value)

    def test_update_creates_versioned_snapshot_and_invalid_ref_keeps_it(self):
        root = self.fixture()
        commit = self.commit_fixture(root)
        self.assertEqual(self.run_watch(root, '--update', '--ref', commit)[0], 0)
        path = root / METRICS.BASELINE
        raw = path.read_bytes()
        self.assertEqual(self.run_watch(root, '--update', '--ref', 'missing-commit')[0], 2)
        self.assertEqual(path.read_bytes(), raw)
        self.assertEqual(self.run_watch(root, '--update')[0], 0)
        self.assertEqual(json.loads(path.read_text())['commit'], commit)

    def test_repository_snapshot_is_readable_and_reported_as_watch(self):
        code, output, error = self.run_watch(ROOT)
        self.assertEqual((code, error), (0, ''))
        self.assertIn('watch only:', output)

    def static_fixture(self):
        root = self.fixture()
        scripts = root / '.github/scripts'
        scripts.mkdir()
        shutil.copy2(ROOT / '.github/scripts/check.sh', scripts / 'check.sh')
        for name in ('a', 'b'):
            package = root / 'src' / name
            package.mkdir()
            (package / 'Cargo.toml').write_text(f'[package]\nname = "{name}"\nversion = "0.1.0"\n')
        (root / 'Cargo.toml').write_text('[workspace]\nmembers = ["src/a", "src/b"]\n')
        policy = {'version': 1, 'owners': {
            name: {'path': 'src/' + name, 'runtime': [], 'build': [], 'dev': []}
            for name in ('a', 'b')}}
        (root / '.github/architecture-policy.json').write_text(json.dumps(policy))
        self.baseline(root)
        tests = root / 'test/ci'
        tests.mkdir(parents=True)
        for script in ('release-package.sh', 'release-publish.sh'):
            (tests / script).write_text('#!/bin/sh\nexit 0\n')
        binaries = root / 'bin'
        binaries.mkdir()
        stub = (
            f'#!{sys.executable}\n'
            'import json, os, pathlib, sys\n'
            'with open(os.environ["WATCH_RECEIPT"], "a") as receipt:\n'
            '    receipt.write(json.dumps(sys.argv[1:]) + "\\n")\n'
            'if sys.argv[1:2] in ([".github/scripts/check-boundary-metrics.py"],\n'
            '                       [".github/scripts/check-architecture.py"]):\n'
            f'    script = pathlib.Path({str(ROOT)!r}) / sys.argv[1]\n'
            '    os.execv(sys.executable, [sys.executable, str(script), "--root", os.getcwd()])\n'
        )
        for name in ('python3', 'node'):
            executable = binaries / name
            executable.write_text(stub)
            executable.chmod(0o700)
        environment = dict(os.environ, PATH=str(binaries) + os.pathsep + os.environ['PATH'],
                           WATCH_RECEIPT=str(root / 'calls.jsonl'))
        return root, policy, environment

    def run_static(self, root, environment):
        return subprocess.run(['bash', str(root / '.github/scripts/check.sh'), 'static'],
                              env=environment, capture_output=True, text=True, check=False)

    def test_static_continues_after_watch_changes_but_rejects_forbidden_dependencies(self):
        root, policy, environment = self.static_fixture()
        (root / METRICS.SOURCES[0] / 'legal.rs').write_text('let joined = values.join(".");\n')
        result = self.run_static(root, environment)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('rose src/engine:join_literal: 0 -> 1', result.stdout)
        calls = [json.loads(line) for line in (root / 'calls.jsonl').read_text().splitlines()]
        self.assertIn(['tools/rust-sources/manage.py', 'verify'], calls)
        self.assertIn(['tools/rust-sources/architecture.py', 'verify'], calls)
        self.assertIn(['tools/rust-sources/documentation.py', 'verify'], calls)
        self.assertIn(['tools/publication-model/selftest.py'], calls)
        self.assertEqual(calls[-1], ['.github/scripts/check-evidence-probes.mjs'])
        with (root / 'src/a/Cargo.toml').open('a') as manifest:
            manifest.write('[target.\'cfg(unix)\'.dependencies]\n'
                           'alias = { package = "b", path = "../b", optional = true }\n')
        result = self.run_static(root, environment)
        self.assertEqual(result.returncode, 1)
        self.assertIn('architecture:', result.stderr)
        policy['owners']['a']['runtime'] = ['b']
        (root / '.github/architecture-policy.json').write_text(json.dumps(policy))
        result = self.run_static(root, environment)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_static_does_not_hide_missing_watch_baseline(self):
        root, _, environment = self.static_fixture()
        (root / METRICS.BASELINE).unlink()
        result = self.run_static(root, environment)
        self.assertEqual(result.returncode, 2)
        self.assertIn('boundary metrics input error:', result.stderr)
        calls = [json.loads(line) for line in (root / 'calls.jsonl').read_text().splitlines()]
        self.assertNotIn(['tools/publication-model/selftest.py'], calls)


if __name__ == '__main__':
    unittest.main()
