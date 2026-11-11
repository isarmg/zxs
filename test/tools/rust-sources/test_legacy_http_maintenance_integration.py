"""Real manager wiring for synthetic HTTP successors; no product fix is installed.

The disposable repository preserves every live source and immutable proof. Only
the unchanged Cargo resolution boundary is stubbed: these comment edits do not
alter manifests or features. This is not a Cargo metadata or behavior test.
"""
from contextlib import contextmanager, redirect_stdout
from copy import deepcopy
import io
import faulthandler
import json
from pathlib import Path
import shutil
import sys
import tempfile
import time
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


def observed(label, operation, *args, **kwargs):
    started = time.monotonic()
    print(f'maintenance phase start: {label}', file=sys.stderr, flush=True)
    try:
        return operation(*args, **kwargs)
    finally:
        elapsed = time.monotonic() - started
        print(f'maintenance phase end: {label}; elapsed={elapsed:.3f}s',
              file=sys.stderr, flush=True)


@contextmanager
def progress_tracebacks():
    faulthandler.dump_traceback_later(60, repeat=True)
    try:
        yield
    finally:
        faulthandler.cancel_dump_traceback_later()


class HttpMaintenanceManager(unittest.TestCase):
    def test_real_manager_successors_preserve_history_and_reject_external_drift(self):
        # Keep this full provenance fixture off small /tmp tmpfs. Do not use
        # links: the test intentionally edits and damages disposable files.
        with progress_tracebacks(), tempfile.TemporaryDirectory(
                prefix='http-maintenance-manager-', dir=ROOT.parent) as temporary:
            root = Path(temporary)
            for name in ('.cargo', '.github', 'compliance', 'docs', 'src', 'helper', 'test', 'tools'):
                observed(f'copy {name}', shutil.copytree, ROOT / name, root / name,
                                ignore=shutil.ignore_patterns('__pycache__', '*.pyc'))
            for name in ('Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'AGENTS.md',
                         '工程规范.md', 'README.md', '.gitignore', '.gitattributes'):
                shutil.copyfile(ROOT / name, root / name)
            catalog = json.loads((root / manage.CATALOG).read_bytes())
            original = deepcopy(catalog)
            original_pins = legacy.RECEIPTS
            original_receipts = {
                root / 'compliance/proof-maintenance' / (sha + '.json.gz'):
                (root / 'compliance/proof-maintenance' / (sha + '.json.gz')).read_bytes()
                for sha in original_pins
            }
            lock_bytes = (root / 'Cargo.lock').read_bytes()
            resolutions = deepcopy(catalog['resolutions'])
            # No source-governance validator is mocked. Fixed resolution data
            # keeps this test focused on the real refresh/verify proof paths.
            with patch.object(legacy, 'RECEIPTS', original_pins), \
                    patch.object(manage, 'resolutions', return_value=resolutions), \
                    patch.object(manage, 'cargo_metadata', side_effect=AssertionError('unexpected Cargo invocation')), \
                    redirect_stdout(io.StringIO()):
                observed('verify', manage.verify, root)
                for old, new in ((b'//! # HTTP transport', b'//! # HTTP transport fixture one'),
                                 (b'//! # HTTP transport fixture one', b'//! # HTTP transport fixture two')):
                    before_bytes = (root / manage.CATALOG).read_bytes()
                    row = observed('successor', successor, root, catalog,
                                   legacy.RECEIPTS[-1], 'src/lib.rs', old, new)
                    receipt = observed('write receipt', write_receipt, root, row)
                    legacy.RECEIPTS = (*legacy.RECEIPTS, receipt)
                    # Source and reviewed pin alone cannot claim a verified
                    # catalog. Only refresh can advance the immediate prefix.
                    with self.assertRaises(ValueError):
                        observed('verify', manage.verify, root)
                    self.assertEqual((root / manage.CATALOG).read_bytes(), before_bytes)
                    observed('refresh', manage.refresh, root)
                    observed('verify', manage.verify, root)
                    catalog = json.loads((root / manage.CATALOG).read_bytes())
                    self.assertEqual(catalog[legacy.KEY], list(legacy.RECEIPTS))
                    committed = (root / manage.CATALOG).read_bytes()
                    observed('refresh', manage.refresh, root)
                    observed('verify', manage.verify, root)
                    self.assertEqual((root / manage.CATALOG).read_bytes(), committed)
                view = observed('load final layout', source_layout.load, root, catalog)
                self.assertEqual(view.legacy_maintenance.receipt_ids, legacy.RECEIPTS)
                restored = observed('project final catalog', view.project_catalog, catalog)
                baseline = observed('project original catalog', view.project_catalog, original)
                self.assertEqual(restored['packages'][legacy.LOGICAL], baseline['packages'][legacy.LOGICAL])
                self.assertEqual(catalog['external_tests'], original['external_tests'])
                for key in source_layout.HISTORY_FIELDS:
                    self.assertEqual(catalog.get(key), original.get(key), key)
                for path, data in original_receipts.items():
                    self.assertEqual(path.read_bytes(), data, path)
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
                        observed(f'reject drift {operation.__name__}', operation, root)
                    self.assertEqual((root / manage.CATALOG).read_bytes(), committed)


if __name__ == '__main__':
    unittest.main()
