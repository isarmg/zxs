"""Behavior of the source-counter ratchet using isolated fixture files."""
import importlib.util
from pathlib import Path
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

    def test_equal_counts_pass_and_both_directions_require_review(self):
        baseline = {'source': {'first': 3, 'second': 5}}
        self.assertEqual(METRICS.changes(baseline, baseline), [])
        self.assertEqual(
            METRICS.changes({'source': {'first': 2, 'second': 5}}, baseline),
            [('source', 'first', 3, 2)],
        )
        self.assertEqual(
            METRICS.changes({'source': {'first': 4, 'second': 1}}, baseline),
            [('source', 'first', 3, 4), ('source', 'second', 5, 1)],
        )

    def test_inventory_changes_require_review(self):
        for current, baseline in [
            ({'new': {'metric': 0}}, {'old': {'metric': 0}}),
            ({'source': {'new': 0}}, {'source': {'old': 0}}),
        ]:
            with self.subTest(current=current):
                with self.assertRaises(ValueError):
                    METRICS.changes(current, baseline)

    def test_missing_source_is_an_error(self):
        with tempfile.TemporaryDirectory() as temporary:
            with self.assertRaises(FileNotFoundError):
                METRICS.counts(Path(temporary))


if __name__ == '__main__':
    unittest.main()
