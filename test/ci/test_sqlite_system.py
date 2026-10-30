"""The system SQLite job must never silently exercise the bundled native build."""
import importlib.util
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('sqlite_system_check', ROOT / '.github/scripts/check-sqlite-system.py')
module = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(module)

class SystemSqlite(unittest.TestCase):
    def metadata(self):
        return {'packages': [{'id': 'native', 'name': 'libsqlite3-sys'}, {'id': 'access', 'name': 'sqlite-access'}],
                'resolve': {'nodes': [{'id': 'native', 'features': ['buildtime_bindgen', 'pkg-config']},
                                      {'id': 'access', 'features': ['sqlite-unbundled']}]}}

    def test_accepts_actual_unbundled_feature_graph(self):
        module.verify_features(self.metadata())

    def test_rejects_all_bundled_native_variants(self):
        for feature in ('bundled', 'bundled-sqlcipher', 'bundled-sqlcipher-vendored-openssl'):
            metadata = self.metadata()
            metadata['resolve']['nodes'][0]['features'].append(feature)
            with self.assertRaises(AssertionError):
                module.verify_features(metadata)

    def test_requires_real_header_binding_generation(self):
        metadata = self.metadata()
        metadata['resolve']['nodes'][0]['features'] = ['pkg-config']
        with self.assertRaises(AssertionError):
            module.verify_features(metadata)

    def test_rejects_unified_access_feature_selection(self):
        metadata = self.metadata()
        metadata['resolve']['nodes'][1]['features'].append('sqlite-bundled')
        with self.assertRaises(AssertionError):
            module.verify_features(metadata)

    def test_workflow_provisions_sdk_only_for_foundation_data(self):
        workflow = (ROOT / '.github/workflows/ci.yml').read_text()
        self.assertIn("name: Install system SQLite SDK for unbundled validation\n        if: env.ZXS_RUN_SOURCE_VALIDATION == 'true' && matrix.suite == 'foundation-data'", workflow)
        self.assertIn('libsqlite3-dev clang libclang-dev pkg-config', workflow)
        self.assertIn("name: Verify isolated system SQLite consumer\n        if: env.ZXS_RUN_SOURCE_VALIDATION == 'true' && matrix.suite == 'foundation-data'", workflow)
        self.assertIn('python3 .github/scripts/check-sqlite-system.py', workflow)

if __name__ == '__main__':
    unittest.main()
