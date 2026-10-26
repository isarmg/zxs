"""Production trimming must not become an unreviewed deletion escape hatch."""
import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / 'tools/rust-sources'))
import external_tests
import source_trimming as trim


class SourceTrimming(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.directory = 'src/foundation/reactive-state'
        (self.root / self.directory / 'src').mkdir(parents=True)
        self.sha = hashlib.sha256(b'old source').hexdigest()
        self.package = {'directory': self.directory, 'origin_directory': 'upstream/reactive',
                        'license_materials': ['unchanged-license'],
                        'files_sha256': {'src/lib.rs': self.sha, 'build.rs': self.sha, 'src/nightly.rs': self.sha}}
        self.previous = {'packages': {self.directory: self.package}}
        self.plan = {'schema': 1, 'packages': [{'directory': self.directory,
            'package': copy.deepcopy(self.package), 'files': [
                {'path': 'build.rs', 'sha256_before': self.sha, 'reason': 'Remove nightly-only build probe'},
                {'path': 'src/nightly.rs', 'sha256_before': self.sha, 'reason': 'Stable compiler only'}]}]}

    def applied(self):
        records, grants = trim.prepare(self.root, self.previous, self.plan)
        catalog = copy.deepcopy(self.previous)
        catalog['packages'][self.directory]['files_sha256'] = {'src/lib.rs': self.sha}
        catalog.update(source_trims=records, source_trim_records_sha256=trim.hashes(records))
        return catalog, grants

    def test_explicit_exact_removal_and_repeated_refresh(self):
        catalog, grants = self.applied()
        trim.validate(self.root, catalog)
        external_tests.check_removed(self.previous['packages'], catalog['packages'], {}, trim_grants=grants)
        records, new = trim.prepare(self.root, catalog)
        self.assertEqual(records, catalog[trim.KEY])
        self.assertEqual(new, {})
        self.assertEqual(self.package['license_materials'], ['unchanged-license'])

    def test_missing_plan_or_unplanned_extra_removal(self):
        catalog, grants = self.applied()
        with self.assertRaises(ValueError):
            external_tests.check_removed(self.previous['packages'], catalog['packages'], {})
        catalog['packages'][self.directory]['files_sha256'] = {}
        with self.assertRaises(ValueError):
            external_tests.check_removed(self.previous['packages'], catalog['packages'], {}, trim_grants=grants)

    def test_bad_plan_mutations(self):
        mutations = [
            lambda p: p['packages'][0]['files'][0].update(sha256_before='0' * 64),
            lambda p: p['packages'][0]['files'][0].update(reason=' '),
            lambda p: p['packages'][0]['package'].update(version='stale'),
            lambda p: p['packages'].append(copy.deepcopy(p['packages'][0])),
            lambda p: p['packages'][0]['files'].append(copy.deepcopy(p['packages'][0]['files'][0])),
            lambda p: p['packages'][0]['files'][0].update(path='../outside.rs'),
            lambda p: p['packages'][0]['files'][0].update(path='Cargo.toml'),
            lambda p: p['packages'][0]['files'][0].update(path='src/tests/test.rs'),
            lambda p: p['packages'][0]['files'][0].update(path='../../../../compliance/external-tests/foundation.json'),
        ]
        for mutate in mutations:
            with self.subTest(mutate=mutate):
                plan = copy.deepcopy(self.plan)
                mutate(plan)
                with self.assertRaises(ValueError):
                    trim.prepare(self.root, self.previous, plan)

    def test_not_removed_or_symlink(self):
        path = self.root / self.directory / 'build.rs'
        path.write_text('still exists')
        with self.assertRaises(ValueError):
            trim.prepare(self.root, self.previous, self.plan)
        path.unlink()
        path.symlink_to(self.root / 'nonexistent')
        with self.assertRaises(ValueError):
            trim.prepare(self.root, self.previous, self.plan)

    def test_receipt_missing_modified_duplicate_or_restored(self):
        catalog, _ = self.applied()
        for field in (trim.KEY, trim.HASHES):
            broken = copy.deepcopy(catalog)
            del broken[field]
            with self.assertRaises(ValueError):
                trim.validate(self.root, broken)
        broken = copy.deepcopy(catalog)
        row = next(iter(broken[trim.KEY].values()))
        row['files'][0]['reason'] = 'changed'
        with self.assertRaises(ValueError):
            trim.validate(self.root, broken)
        broken[trim.HASHES] = trim.hashes(broken[trim.KEY])
        with self.assertRaises(ValueError):
            trim.validate(self.root, broken)
        duplicate = copy.deepcopy(next(iter(catalog[trim.KEY].values())))
        duplicate['files'][0]['reason'] = 'second record'
        broken = copy.deepcopy(catalog)
        broken[trim.KEY][trim.receipt_id(duplicate)] = duplicate
        broken[trim.HASHES] = trim.hashes(broken[trim.KEY])
        with self.assertRaises(ValueError):
            trim.validate(self.root, broken)
        (self.root / self.directory / 'build.rs').write_text('restored')
        with self.assertRaises(ValueError):
            trim.validate(self.root, catalog)

    def test_rehashed_forged_origin_metadata_is_rejected(self):
        catalog, _ = self.applied()
        row = copy.deepcopy(next(iter(catalog[trim.KEY].values())))
        row['package']['license_materials'] = ['forged-license']
        catalog[trim.KEY] = {trim.receipt_id(row): row}
        catalog[trim.HASHES] = trim.hashes(catalog[trim.KEY])
        with self.assertRaisesRegex(ValueError, 'stable origin metadata'):
            trim.validate(self.root, catalog)

    def test_append_only_and_stale_repeat(self):
        catalog, _ = self.applied()
        with self.assertRaises(ValueError):
            trim.check_append_only(catalog, self.previous)
        with self.assertRaises(ValueError):
            trim.prepare(self.root, catalog, self.plan)

    def test_merged_consolidated_and_retired_owners_rejected(self):
        for extra in ({'merges': [{'directory': self.directory}]},
                      {'consolidations': {'x': {'destination_directory': self.directory}}},
                      {'packages': {}}):
            previous = dict(self.previous, **extra)
            with self.assertRaises(ValueError):
                trim.prepare(self.root, previous, self.plan)

    def test_external_fragment_receipts_remain_required(self):
        owner = {'files': [{'source': self.directory + '/build.rs',
                           'source_sha256_before': self.sha, 'path': 'test/build.rs'}]}
        with self.assertRaises(ValueError):
            external_tests.check_previous({self.directory: owner}, {})



class ImportedSourceTrimming(SourceTrimming):
    def setUp(self):
        super().setUp()
        self.directory = 'src/foundation/web-runtime/wasm-text-parser'
        self.package['directory'] = self.directory
        self.previous['packages'] = {self.directory: self.package}
        self.plan['packages'][0]['directory'] = self.directory
        self.plan['packages'][0]['package'] = copy.deepcopy(self.package)
        (self.root / self.directory / 'src').mkdir(parents=True)
        from unittest.mock import patch
        self.authenticated = patch.object(trim.source_layout, 'imported_directories',
                                          return_value={self.directory})
        self.authenticated.start()
        self.addCleanup(self.authenticated.stop)

    def test_grouped_owner_without_authenticated_import_is_rejected(self):
        from unittest.mock import patch
        with patch.object(trim.source_layout, 'imported_directories', return_value=set()):
            with self.assertRaisesRegex(ValueError, 'standalone foundation owner'):
                trim.prepare(self.root, self.previous, self.plan)

    def test_layout_keeps_old_grouped_trim_closed(self):
        from types import SimpleNamespace
        layout = SimpleNamespace(imported_owners={self.directory})
        trim.source_layout.Layout.require_supported_plan(layout, self.previous, trim=self.plan)
        bad = copy.deepcopy(self.plan)
        bad['packages'][0]['directory'] = 'src/foundation/core-types/old-owner'
        with self.assertRaisesRegex(ValueError, 'authenticated imported owner'):
            trim.source_layout.Layout.require_supported_plan(layout, self.previous, trim=bad)

    def test_layout_checks_current_trim_hash_absence_and_nonoverlap(self):
        from types import SimpleNamespace
        catalog, _ = self.applied()
        layout = SimpleNamespace(root=self.root, imported_owners={self.directory})
        check = trim.source_layout.Layout._check_imported_trims
        self.assertEqual(check(layout, catalog), set(catalog[trim.KEY]))
        key = next(iter(catalog[trim.KEY]))
        bad = copy.deepcopy(catalog)
        bad[trim.HASHES][key] = '0' * 64
        with self.assertRaisesRegex(ValueError, 'invalid imported source trim'):
            check(layout, bad)
        bad = copy.deepcopy(catalog)
        bad['packages'][self.directory]['files_sha256']['build.rs'] = self.sha
        with self.assertRaisesRegex(ValueError, 'invalid imported source trim'):
            check(layout, bad)
        duplicate = copy.deepcopy(catalog[trim.KEY][key])
        duplicate['files'][0]['reason'] = 'duplicate grant'
        key2 = trim.receipt_id(duplicate)
        bad = copy.deepcopy(catalog)
        bad[trim.KEY][key2] = duplicate
        bad[trim.HASHES][key2] = key2
        with self.assertRaisesRegex(ValueError, 'invalid imported source trim'):
            check(layout, bad)

    def test_imported_trim_does_not_reenter_predecessor_layout(self):
        from types import SimpleNamespace
        from unittest.mock import patch
        with patch.object(trim.source_layout, 'physical', side_effect=AssertionError('historical reentry')):
            catalog, _ = self.applied()
            layout = SimpleNamespace(root=self.root, imported_owners={self.directory})
            self.assertEqual(trim.source_layout.Layout._check_imported_trims(layout, catalog),
                             set(catalog[trim.KEY]))


if __name__ == '__main__':
    unittest.main()
