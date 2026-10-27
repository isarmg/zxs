"""Real manager wiring for synthetic HTTP successors; no product fix is installed.

The disposable repository preserves every live source and immutable proof. Only
the unchanged Cargo resolution boundary is stubbed: these comment edits do not
alter manifests or features. This is not a Cargo metadata or behavior test.
"""
from contextlib import redirect_stdout
from copy import deepcopy
import io
import json
from pathlib import Path
import shutil
import sys
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(ROOT / 'tools/rust-sources'))
import manage
import proof_legacy_maintenance as legacy
import proof_routing as routing
import source_layout
from test_legacy_http_maintenance import successor, write_receipt


class HttpMaintenanceManager(unittest.TestCase):
    def test_real_manager_successors_preserve_history_and_reject_external_drift(self):
        # Keep this full provenance fixture off small /tmp tmpfs. Do not use
        # links: the test intentionally edits and damages disposable files.
        with tempfile.TemporaryDirectory(prefix='http-maintenance-manager-', dir=ROOT.parent) as temporary:
            root = Path(temporary)
            for name in ('.cargo', '.github', 'compliance', 'docs', 'src', 'test', 'tools'):
                shutil.copytree(ROOT / name, root / name,
                                ignore=shutil.ignore_patterns('__pycache__', '*.pyc'))
            for name in ('Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'AGENTS.md',
                         '禁止防御性编程.md', 'README.md', '.gitignore', '.gitattributes'):
                shutil.copyfile(ROOT / name, root / name)
            catalog = json.loads((root / manage.CATALOG).read_bytes())
            original = deepcopy(catalog)
            original_pins = legacy.RECEIPTS
            first = root / 'compliance/proof-maintenance' / (original_pins[0] + '.json.gz')
            first_bytes = first.read_bytes()
            lock_bytes = (root / 'Cargo.lock').read_bytes()
            resolutions = deepcopy(catalog['resolutions'])
            # No source-governance validator is mocked. Fixed resolution data
            # keeps this test focused on the real refresh/verify proof paths.
            with patch.object(legacy, 'RECEIPTS', original_pins), \
                    patch.object(manage, 'resolutions', return_value=resolutions), \
                    patch.object(manage, 'cargo_metadata', side_effect=AssertionError('unexpected Cargo invocation')), \
                    redirect_stdout(io.StringIO()):
                manage.verify(root)
                for old, new in ((b'//! # HTTP transport', b'//! # HTTP transport fixture one'),
                                 (b'//! # HTTP transport fixture one', b'//! # HTTP transport fixture two')):
                    before_bytes = (root / manage.CATALOG).read_bytes()
                    row = successor(root, catalog, legacy.RECEIPTS[-1], 'src/lib.rs', old, new)
                    receipt = write_receipt(root, row)
                    legacy.RECEIPTS = (*legacy.RECEIPTS, receipt)
                    # Source and reviewed pin alone cannot claim a verified
                    # catalog. Only refresh can advance the immediate prefix.
                    with self.assertRaises(ValueError):
                        manage.verify(root)
                    self.assertEqual((root / manage.CATALOG).read_bytes(), before_bytes)
                    manage.refresh(root)
                    manage.verify(root)
                    catalog = json.loads((root / manage.CATALOG).read_bytes())
                    self.assertEqual(catalog[legacy.KEY], list(legacy.RECEIPTS))
                    committed = (root / manage.CATALOG).read_bytes()
                    manage.refresh(root)
                    manage.verify(root)
                    self.assertEqual((root / manage.CATALOG).read_bytes(), committed)
                view = source_layout.load(root, catalog)
                self.assertEqual(view.legacy_maintenance.receipt_ids, legacy.RECEIPTS)
                restored = view.project_catalog(catalog)
                baseline = view.project_catalog(original)
                self.assertEqual(restored['packages'][legacy.LOGICAL], baseline['packages'][legacy.LOGICAL])
                self.assertEqual(catalog['external_tests'], original['external_tests'])
                for key in source_layout.HISTORY_FIELDS:
                    self.assertEqual(catalog.get(key), original.get(key), key)
                self.assertEqual(first.read_bytes(), first_bytes)
                self.assertEqual((root / 'Cargo.lock').read_bytes(), lock_bytes)
                for relative, digest in original['provenance_sha256'].items():
                    self.assertEqual(routing.digest((root / relative).read_bytes()), digest, relative)
                corrected = root / ('test/foundation/http-transport/adapters/unit/src/server/'
                                    'conn/auto/mod/guarded_tests_1135.rs')
                self.assertIn(corrected.relative_to(root).as_posix(), view.corrections.rows)
                corrected.write_bytes(corrected.read_bytes() + b'// unsupported external edit\n')
                committed = (root / manage.CATALOG).read_bytes()
                for operation in (manage.verify, manage.refresh):
                    with self.assertRaises(ValueError):
                        operation(root)
                    self.assertEqual((root / manage.CATALOG).read_bytes(), committed)


if __name__ == '__main__':
    unittest.main()
