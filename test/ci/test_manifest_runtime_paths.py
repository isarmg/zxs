"""Finite runtime-path inventory for explicit Cargo test/bench entrypoints."""
import importlib.util
import json
from pathlib import Path
import runpy
import tempfile
import tomllib
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
PATHS = runpy.run_path(str(ROOT / '.github/scripts/manifest_runtime_paths.py'))


class ManifestRuntimePaths(unittest.TestCase):
    def test_declared_target_static_references_match_reviewed_inventory(self):
        expected = json.loads((ROOT / 'test/ci/fixtures/manifest-runtime-paths.json').read_text())
        self.assertEqual(PATHS['inventory'](ROOT), expected)
        self.assertEqual(len(expected), 11)
        self.assertEqual({row['source'] for row in expected}, {
            'test/foundation/compression/streams/tests/gunzip.rs',
            'test/schema-validation/canonical.rs',
            'test/tools/contract-check/gitignore.rs',
            'test/tools/contract-check/source_export.rs',
            'test/foundation/unicode-display-width/tests/tests.rs',
            'test/foundation/wasm-text-parser/tests/parse-fail.rs',
        })

    def test_recognized_forms_ignore_comments_and_quoted_examples(self):
        concat = 'concat!(env!("CARGO_MANIFEST_DIR"), "/fixture.json")'
        join = 'std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixture",)'
        source = ('// ' + concat + '\n/* outer /* nested */ ' + join + ' */\n'
                  'let quoted = ' + json.dumps(concat) + ';\n'
                  'let raw = r###"' + join + '"###;\n'
                  'let real = ' + concat + ';\nlet path = ' + join + ';\n'
                  'let dynamic = Path::new(env!("CARGO_MANIFEST_DIR")).join(variable);')
        self.assertEqual(PATHS['references'](source),
                         [('concat', '/fixture.json'), ('join', 'fixture')])

    def test_declared_test_and_bench_use_manifest_owner_not_source_parent(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            owner = 'src/foundation/example/provider'
            package = root / owner
            package.mkdir(parents=True)
            (root / 'compliance').mkdir()
            (root / 'compliance/rust-sources.json').write_text(json.dumps({'packages': {owner: {}}}))
            (root / 'Cargo.toml').write_text('[workspace]\nmembers = []\n')
            (package / 'Cargo.toml').write_text(
                '[package]\nname="provider"\nversion="0.0.0"\n'
                '[[test]]\nname="fixture"\npath="../../../../test/fixture.rs"\n'
                '[[bench]]\nname="fixture"\npath="../../../../test/bench.rs"\n')
            (root / 'test').mkdir()
            (root / 'test/input.json').write_text('[]')
            source = 'PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../test/input.json")'
            for name in ('fixture', 'bench'):
                (root / 'test' / (name + '.rs')).write_text(source)
            rows = PATHS['inventory'](root)
            self.assertEqual({row['kind'] for row in rows}, {'test', 'bench'})
            self.assertEqual({row['destination'] for row in rows}, {'test/input.json'})
            (root / 'test/fixture.rs').write_text(source.replace('../../../../test/', '../../../test/'))
            with self.assertRaisesRegex(ValueError, 'missing manifest-relative input'):
                PATHS['inventory'](root)

    def assert_canonical_suite(self, manifest):
        data = tomllib.loads(manifest.read_text())
        target = next(row for row in data['test'] if row['name'] == 'canonical')
        source = (manifest.parent / target['path']).resolve()
        references = PATHS['references'](source.read_text())
        self.assertEqual(len(references), 1)
        form, literal = references[0]
        self.assertEqual(form, 'join')
        destination = PATHS['resolve_reference'](manifest, form, literal)
        self.assertTrue(destination.is_dir(), str(destination))
        files = sorted(destination.glob('*.json'))
        self.assertEqual(len(files), 31)
        cases = [json.loads(path.read_text()) for path in files]
        self.assertTrue(all(isinstance(items, list) for items in cases))
        self.assertEqual(sum(map(len, cases)), 1509)
        # The actual Rust test keeps both assertions; finding files is not a
        # substitute for executing those canonicalization cases in the CI gate.
        self.assertIn('assert_eq!(files.len(), 31);', source.read_text())
        self.assertIn('assert_eq!(count, 1509);', source.read_text())

    def test_schema_canonical_suite_resolves_from_actual_registered_owner(self):
        self.assert_canonical_suite(ROOT / 'src/foundation/data-storage/schema-validation/Cargo.toml')

    def test_schema_isolated_mirror_keeps_real_canonical_fixture_inputs(self):
        spec = importlib.util.spec_from_file_location(
            'schema_validation_gate', ROOT / '.github/scripts/check-schema-validation.py')
        gate = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(gate)

        class CapturedBeforeCargo(Exception):
            pass

        def inspect(manifest):
            self.assert_canonical_suite(manifest)
            raise CapturedBeforeCargo

        with mock.patch.object(gate, 'check', side_effect=inspect), \
                mock.patch.object(gate.subprocess, 'run') as cargo:
            with self.assertRaises(CapturedBeforeCargo):
                gate.main()
            cargo.assert_not_called()


if __name__ == '__main__':
    unittest.main()
