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
            self.assertEqual(result[METRICS.SOURCES[0]]['validate_id'], 0)
            self.assertEqual(set(result), set(METRICS.SOURCES))

    def test_equal_and_lower_counts_pass_but_rises_are_reported(self):
        baseline = {'source': {'first': 3, 'second': 5}}
        self.assertEqual(METRICS.increases(baseline, baseline), [])
        self.assertEqual(METRICS.increases({'source': {'first': 2, 'second': 5}}, baseline), [])
        self.assertEqual(
            METRICS.increases({'source': {'first': 4, 'second': 1}}, baseline),
            [('source', 'first', 3, 4)],
        )

    def test_inventory_changes_require_review(self):
        for current, baseline in [
            ({'new': {'metric': 0}}, {'old': {'metric': 0}}),
            ({'source': {'new': 0}}, {'source': {'old': 0}}),
        ]:
            with self.subTest(current=current):
                with self.assertRaises(ValueError):
                    METRICS.increases(current, baseline)

    def test_missing_source_is_an_error(self):
        with tempfile.TemporaryDirectory() as temporary:
            with self.assertRaises(FileNotFoundError):
                METRICS.counts(Path(temporary))


if __name__ == '__main__':
    unittest.main()
