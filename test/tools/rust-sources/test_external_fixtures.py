"""Imported Unicode vectors cannot be misattributed to this project or package."""
import copy
import hashlib
import json
from pathlib import Path
import shutil
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'tools/rust-sources'))
import external_fixtures
import external_tests


class ImportedFixtureAccounting(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.path = 'test/naming/data/normalization-test-16.0.0.txt'
        self.receipt_path, _ = external_fixtures.RECEIPTS[self.path]
        self.receipt = json.loads((ROOT / self.receipt_path).read_bytes())
        self.owner = self.receipt['directory']
        self.packages = {self.owner: {
            'origin_directory': self.receipt['owner_origin'], 'files_sha256': {},
        }}
        mapping = json.loads((ROOT / 'compliance/external-tests/integrated.json').read_bytes())
        naming = next(owner for owner in mapping['owners'] if owner['directory'] == self.owner)
        self.row = copy.deepcopy(next(row for row in naming['files'] if row['path'] == self.path))
        self.record = {'schema_version': 1, 'owners': [{
            'directory': self.owner, 'roots': ['test/naming/data'], 'files': [self.row],
        }]}
        for relative in (self.path, self.receipt_path, self.receipt['license_path']):
            destination = self.root / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / relative, destination)
        self.mapping = self.root / 'compliance/external-tests/imported.json'
        self.mapping.parent.mkdir(parents=True)
        self.save()

    def save(self):
        self.mapping.write_text(json.dumps(self.record))

    def load(self):
        return external_tests.load(self.root, self.packages)[0]

    def test_reviewed_receipt_preserves_separate_upstream_fixture_identity(self):
        owners = self.load()
        external_tests.check_previous({}, owners, self.root)
        self.assertEqual(owners[self.owner]['files'], [self.row])
        self.assertEqual(self.row['source_kind'], 'upstream-fixture')
        self.assertEqual(self.row['source'], self.receipt['source'])
        self.assertEqual(self.receipt['upstream'], 'Unicode Consortium')
        self.assertEqual(self.receipt['license'], 'Unicode-3.0')
        self.assertIn(self.receipt['revision'], self.receipt['retrieved_url'])
        self.assertEqual(self.row['sha256'], self.receipt['sha256'])

    def test_registered_fixture_cannot_be_claimed_as_project_authored(self):
        self.row.update(source_kind='project-authored', source=self.path)
        self.row.pop('source_receipt')
        self.save()
        with self.assertRaisesRegex(ValueError, 'ownership'):
            self.load()

    def test_mapping_cannot_change_source_receipt_owner_or_pinned_bytes(self):
        original = copy.deepcopy(self.row)
        for field, value in [
            ('source_kind', 'unknown'),
            ('source', 'unicodetools/data/ucd/17.0.0/NormalizationTest.txt'),
            ('source_receipt', 'compliance/fixtures/unreviewed.json'),
            ('source_sha256_before', 'a' * 64),
            ('sha256', 'a' * 64),
            ('origin_directory', 'vendor/other'),
        ]:
            with self.subTest(field=field):
                self.row.clear()
                self.row.update(original)
                self.row[field] = value
                self.save()
                with self.assertRaisesRegex(ValueError, 'ownership'):
                    self.load()

    def test_fixture_content_drift_cannot_be_accepted_by_updating_current_hash(self):
        fixture = self.root / self.path
        fixture.write_bytes(b'not the Unicode normalization vectors')
        with self.assertRaisesRegex(ValueError, 'inventory/content drift'):
            self.load()
        changed = hashlib.sha256(fixture.read_bytes()).hexdigest()
        self.row.update(sha256=changed, source_sha256_before=changed)
        self.save()
        with self.assertRaisesRegex(ValueError, 'ownership'):
            self.load()

    def test_source_revision_urls_and_license_are_immutable_receipt_bytes(self):
        path = self.root / self.receipt_path
        for field in ('revision', 'official_url', 'retrieved_url', 'license_path'):
            with self.subTest(field=field):
                changed = dict(self.receipt, **{field: 'unreviewed'})
                path.write_text(json.dumps(changed))
                with self.assertRaisesRegex(ValueError, 'fixture receipt changed'):
                    self.load()

    def test_missing_or_linked_receipt_and_changed_legal_material_fail(self):
        receipt = self.root / self.receipt_path
        original = receipt.read_bytes()
        receipt.unlink()
        with self.assertRaises(FileNotFoundError):
            self.load()
        receipt.symlink_to(self.root / self.receipt['license_path'])
        with self.assertRaisesRegex(ValueError, 'symlink'):
            self.load()
        receipt.unlink()
        receipt.write_bytes(original)
        license_file = self.root / self.receipt['license_path']
        license_file.write_bytes(b'changed license')
        with self.assertRaisesRegex(ValueError, 'fixture license changed'):
            self.load()
        license_file.unlink()
        with self.assertRaises(FileNotFoundError):
            self.load()

    def test_fixture_cannot_move_to_another_owner_or_use_transfer_grants(self):
        owners = self.load()
        origin = self.packages[self.owner]['origin_directory']
        grant = (origin, self.row['source'], self.row['source_sha256_before'])
        with self.assertRaisesRegex(ValueError, 'ownership'):
            external_tests.load(self.root, self.packages, transfers={self.owner: {grant}})
        wrong = 'src/other'
        self.record['owners'][0]['directory'] = wrong
        self.packages[wrong] = self.packages.pop(self.owner)
        self.save()
        with self.assertRaisesRegex(ValueError, 'ownership'):
            self.load()
        with self.assertRaisesRegex(ValueError, 'accounting shrank'):
            external_tests.check_previous(owners, {wrong: owners[self.owner]}, self.root)

    def test_fixture_addition_cannot_dispose_of_existing_package_sources(self):
        owners = self.load()
        previous = {self.owner: dict(self.packages[self.owner], files_sha256={
            'test.rs': self.row['source_sha256_before'],
        })}
        with self.assertRaisesRegex(ValueError, 'unaccounted source removal'):
            external_tests.check_removed(previous, self.packages, owners)

    def test_established_fixture_identity_cannot_disappear_or_be_rewritten(self):
        previous = self.load()
        with self.assertRaisesRegex(ValueError, 'accounting shrank'):
            external_tests.check_previous(previous, {}, self.root)
        for field, value in [('source_kind', 'project-authored'),
                             ('source_receipt', 'compliance/fixtures/other.json'),
                             ('source', self.path), ('source_sha256_before', 'a' * 64)]:
            with self.subTest(field=field):
                current = copy.deepcopy(previous)
                current[self.owner]['files'][0][field] = value
                with self.assertRaisesRegex(ValueError, 'accounting shrank'):
                    external_tests.check_previous(previous, current, self.root)

    def test_import_support_does_not_rewrite_an_existing_local_identity(self):
        current = self.load()
        previous = copy.deepcopy(current)
        row = previous[self.owner]['files'][0]
        row.update(source_kind='project-authored', source=self.path)
        row.pop('source_receipt')
        with self.assertRaisesRegex(ValueError, 'accounting shrank'):
            external_tests.check_previous(previous, current, self.root)

    def test_archived_fixture_still_requires_source_receipt_and_original_license(self):
        owner = self.load()[self.owner]
        package = dict(self.packages[self.owner], directory=self.owner)
        (self.root / self.path).unlink()
        (self.root / 'test/naming/data').rmdir()
        external_tests.validate_archived(self.root, owner, package, removed=True)
        changed = copy.deepcopy(owner)
        changed['files'][0]['source_kind'] = 'project-authored'
        with self.assertRaisesRegex(ValueError, 'archived external test source'):
            external_tests.validate_archived(self.root, changed, package, removed=True)
        (self.root / self.receipt['license_path']).write_bytes(b'changed')
        with self.assertRaisesRegex(ValueError, 'fixture license changed'):
            external_tests.validate_archived(self.root, owner, package, removed=True)


if __name__ == '__main__':
    unittest.main()
