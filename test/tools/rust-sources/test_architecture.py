"""Consumer index must distinguish declaration edges from selected closures."""
import importlib.util
from copy import deepcopy
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location('architecture', (Path(__file__).resolve().parents[3] / "tools/rust-sources" / 'architecture.py'))
architecture = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(architecture)


class ConsumerEdges(unittest.TestCase):
    def test_root_rescue_is_indexed_as_a_foundation_consumer(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'Cargo.toml').write_text(
                '[workspace]\nmembers = ["rescue"]\n'
                '[workspace.dependencies]\nmini = { path = "src/foundation/mini" }\n',
                encoding='utf-8')
            (root / 'rescue').mkdir()
            (root / 'rescue/Cargo.toml').write_text(
                '[package]\nname = "storage-rescue"\nversion = "1.0.0"\n'
                '[dependencies]\nmini.workspace = true\n', encoding='utf-8')
            documents = architecture.manifests(root)
            self.assertEqual(set(documents), {'Cargo.toml', 'rescue/Cargo.toml'})
            self.assertEqual(architecture.consumer_edges(root, documents)['src/foundation/mini'],
                             ['rescue [normal]'])

    def test_merged_source_index_keeps_identity_without_reprinting_history(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'Cargo.toml').write_text('[workspace]\nmembers = []\n', encoding='utf-8')
            catalog = {
                'packages': {}, 'resolutions': [],
                'merges': [{
                    'directory': 'src/foundation/current-owner',
                    'origin_directory': 'vendor/original-1.0',
                    'name': 'current-owner', 'version': '1.0',
                    'upstream': {'name': 'original', 'version': '1.0'},
                    'changes': '2026-10-20 implementation review: docs/reports/old-review.md',
                }],
            }
            original = deepcopy(catalog)
            document = architecture.document(root, catalog, {'profiles': []})
            self.assertIn('| original@1.0 | src/foundation/current-owner |', document)
            self.assertNotIn('2026-10-20 implementation review', document)
            self.assertNotIn('docs/reports/', document)
            self.assertEqual(catalog, original, 'rendering must not rewrite provenance')

    def test_generated_product_roles_use_the_current_rescue_entrypoint(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'Cargo.toml').write_text('[workspace]\nmembers = []\n', encoding='utf-8')
            document = architecture.document(root, {'packages': {}}, {'profiles': []})
            self.assertIn('engine / server / client / rescue]', document)
            self.assertNotIn('client / admin]', document)

    def test_inherited_alias_target_optional_and_dev_edges_remain_visible(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            documents = {
                'Cargo.toml': {'workspace': {'dependencies': {'renamed': {'path': 'src/foundation/mini', 'package': 'mini'}}}},
                'src/app/Cargo.toml': {'dependencies': {'renamed': {'workspace': True, 'optional': True}},
                    'dev-dependencies': {'mini': {'path': '../foundation/mini'}},
                    'target': {'cfg(unix)': {'build-dependencies': {'mini': {'path': '../foundation/mini', 'optional': True}}}}},
            }
            edges = architecture.consumer_edges(root, documents)['src/foundation/mini']
            self.assertEqual(edges, ['src/app [build, optional, cfg(unix)]', 'src/app [dev]', 'src/app [normal, optional] alias=renamed'])
            self.assertFalse(any('Cargo.toml' in edge for edge in edges))


if __name__ == '__main__':
    unittest.main()
