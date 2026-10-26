"""Cargo artifact admission for the retired-walker differential fixture."""
import json
from pathlib import Path
import runpy
import unittest

ROOT = Path(__file__).resolve().parents[2]
LIBRARY = runpy.run_path(str(ROOT / 'test/tools/contract-check/check_document_scan.py'))['tempfile_library']


class OracleArtifact(unittest.TestCase):
    def artifact(self, name='tempfile'):
        return json.dumps({'reason': 'compiler-artifact', 'target': {'name': name, 'kind': ['lib']},
                           'filenames': [f'/target/deps/lib{name}-current.rlib', '/target/deps/current.rmeta']})

    def test_exact_reported_library_ignores_other_crates_and_metadata(self):
        self.assertEqual(LIBRARY(self.artifact('other') + '\n' + self.artifact()),
                         Path('/target/deps/libtempfile-current.rlib'))

    def test_missing_or_ambiguous_library_is_not_a_target_search(self):
        for output in ['', self.artifact('other'), self.artifact() + '\n' + self.artifact()]:
            with self.subTest(output=output), self.assertRaises(ValueError):
                LIBRARY(output)

    def test_compiler_diagnostics_are_not_hidden_by_artifact_selection(self):
        for level in ['warning', 'error']:
            output = json.dumps({'reason': 'compiler-message',
                                 'message': {'level': level, 'message': 'specific cause'}})
            with self.subTest(level=level), self.assertRaisesRegex(ValueError, 'specific cause'):
                LIBRARY(output + '\n' + self.artifact())


if __name__ == '__main__':
    unittest.main()
