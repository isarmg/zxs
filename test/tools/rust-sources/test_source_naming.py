"""Explicit source renames preserve exact source and test provenance."""
from copy import deepcopy
import hashlib
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

TOOLS = Path(__file__).resolve().parents[3] / 'tools/rust-sources'
sys.path.insert(0, str(TOOLS))
import source_naming as naming
import proof_routing


def digest(data):
    return hashlib.sha256(data).hexdigest()


def inventory(path):
    if path.is_symlink():
        raise ValueError('symlink')
    result = {}
    for file in path.rglob('*'):
        if file.is_symlink():
            raise ValueError('symlink')
        if file.is_file():
            result[file.relative_to(path).as_posix()] = digest(file.read_bytes())
    return result


class SourceNaming(unittest.TestCase):
    def setUp(self):
        import proof_maintenance
        maintenance = patch.object(proof_maintenance, 'RECEIPTS', ())
        maintenance.start(); self.addCleanup(maintenance.stop)
        routing = patch.object(proof_routing, "RECEIPTS", ())
        routing.start()
        self.addCleanup(routing.stop)
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.old = 'src/foundation/mini-1.0.0'
        self.new = 'src/foundation/value-core'
        self.test_old = 'test/foundation/mini-1.0.0'
        self.test_new = 'test/foundation/value-core'
        for path in (self.new, self.test_new, 'compliance'):
            (self.root / path).mkdir(parents=True)
        (self.root / self.new / 'Cargo.toml').write_text('[package]\nname = "value-core"\nversion = "1.0.0"\n')
        (self.root / self.new / 'lib.rs').write_text('pub fn value() {}\n')
        (self.root / self.test_new / 'test.rs').write_text('value_core::value();\n')
        old_files = {'Cargo.toml': digest(b'[package]\nname = "mini"\nversion = "1.0.0"\n'),
                     'lib.rs': digest(b'pub fn mini() {}\n')}
        self.package = dict(directory=self.old, name='mini', version='1.0.0',
                            origin_directory='vendor/mini-1.0.0', upstream={'name': 'mini', 'version': '1.0.0'}, files_sha256=old_files)
        before_bytes = b'mini::mini();\n'
        self.owner = dict(directory=self.old, origin_directory=self.package['origin_directory'],
                          roots=[self.test_old], files=[dict(source=self.old + '/tests/test.rs',
                          path=self.test_old + '/test.rs', source_sha256_before=digest(before_bytes), sha256=digest(before_bytes))],
                          file_count=1, bytes=len(before_bytes))
        after_owner = deepcopy(self.owner)
        after_owner.update(directory=self.new, roots=[self.test_new], bytes=(self.root / self.test_new / 'test.rs').stat().st_size)
        after_owner['files'][0].update(path=self.test_new + '/test.rs', sha256=digest((self.root / self.test_new / 'test.rs').read_bytes()))
        self.previous = dict(packages={self.old: self.package}, external_tests={self.old: self.owner})
        self.item = dict(directory=self.old, package=deepcopy(self.package), destination_directory=self.new,
                         destination_name='value-core', reason='Functional names without release suffixes',
                         files_sha256=inventory(self.root / self.new), external_tests=deepcopy(self.owner),
                         destination_external_tests=after_owner)
        self.plan = dict(schema=1, packages=[self.item])

    def apply(self):
        return naming.prepare(self.root, self.previous, self.plan, inventory)

    def reject(self, pattern=None):
        with self.assertRaisesRegex(ValueError, pattern or '.'):
            self.apply()

    def test_prepare_preserves_newer_catalog_schema(self):
        self.previous['schema'] = 8
        with patch.object(naming, 'validate_history', wraps=naming.validate_history) as validate:
            self.apply()
        self.assertGreaterEqual(len(validate.call_args_list), 2)
        for call in validate.call_args_list:
            self.assertEqual(call.args[1]['schema'], 8)

    def test_exact_reviewed_move_and_content_edits(self):
        baseline = deepcopy(self.previous)
        active, records = self.apply()
        self.assertEqual(self.previous, baseline)
        self.assertEqual(set(active), {self.new})
        self.assertEqual(records[self.old]['package'], self.package)
        self.assertEqual(records[self.old]['external_tests'], self.owner)
        self.assertEqual(active[self.new]['origin_directory'], self.package['origin_directory'])
        self.assertEqual(naming.source_prefixes(records, self.new, self.package['origin_directory']), (self.new + '/', self.old + '/'))
        self.assertEqual(naming.source_prefixes(records, self.new, 'other'), (self.new + '/',))
        naming.check_current_external(self.previous, {self.new: self.item['destination_external_tests']}, records)
        catalog = dict(schema=5, packages=active, renames=records, rename_records_sha256=naming.hashes(records))
        self.assertEqual(naming.validate_history(self.root, catalog), records)
        # History is not a permanent content freeze; ordinary explicit refresh
        # may record later source edits without rewriting this transition.
        (self.root / self.new / 'lib.rs').write_text('pub fn later() {}\n')
        naming.validate_history(self.root, catalog)
        self.assertEqual(naming.prepare(self.root, catalog, None, inventory), (active, records))

    def test_no_plan_does_not_learn_new_directory(self):
        active, records = naming.prepare(self.root, self.previous, None, inventory)
        self.assertEqual(active, self.previous['packages'])
        self.assertEqual(records, {})

    def test_before_package_must_match(self):
        self.item['package']['name'] = 'other'
        self.reject('previous receipt')

    def test_before_external_must_match(self):
        self.item['external_tests']['bytes'] += 1
        self.reject('previous receipt')

    def test_missing_or_extra_production_files(self):
        for name, action in [('lib.rs', 'remove'), ('extra.rs', 'add')]:
            with self.subTest(action=action):
                old = deepcopy(self.item['files_sha256'])
                if action == 'remove':
                    self.item['files_sha256'].pop(name)
                else:
                    self.item['files_sha256'][name] = 'a' * 64
                self.reject('file inventory changed')
                self.item['files_sha256'] = old

    def test_unreviewed_source_edits(self):
        (self.root / self.new / 'lib.rs').write_text('different')
        self.reject('content differs')

    def test_unreviewed_external_edits(self):
        (self.root / self.test_new / 'test.rs').write_text('different')
        self.reject('external content differs')

    def test_missing_external_fragment(self):
        self.item['destination_external_tests']['files'] = []
        self.reject('fragment inventory changed')

    def test_historical_source_rewrite_rejected(self):
        self.item['destination_external_tests']['files'][0]['source'] = self.new + '/tests/test.rs'
        self.reject('historical source')

    def test_historical_hash_rewrite_rejected(self):
        self.item['destination_external_tests']['files'][0]['source_sha256_before'] = 'a' * 64
        self.reject('historical source')

    def test_old_source_root_cannot_remain(self):
        (self.root / self.old).mkdir()
        self.reject('absent old')

    def test_old_test_root_cannot_remain(self):
        (self.root / self.test_old).mkdir()
        self.reject('old external test root')

    def test_manifest_version_not_upgrade(self):
        path = self.root / self.new / 'Cargo.toml'
        path.write_text(path.read_text().replace('1.0.0', '2.0.0'))
        self.item['files_sha256'] = inventory(self.root / self.new)
        self.reject('manifest identity/version')

    def test_duplicate_plan_rejected(self):
        self.plan['packages'].append(deepcopy(self.item))
        self.reject('duplicate')

    def test_unsafe_paths_rejected(self):
        for target in ('../escape', '/tmp/escape', 'src/foundation/../escape', 'src/foundation/a/b'):
            with self.subTest(target=target):
                self.item['destination_directory'] = target
                self.reject()

    def test_symlink_rejected(self):
        (self.root / self.new / 'link').symlink_to('/tmp')
        self.reject('symlink')

    def test_upgraded_source_rejected(self):
        self.previous['superseded'] = {'earlier': {'package': self.package}}
        self.reject('chain-aware')

    def test_pinned_current_source_rejected(self):
        (self.root / 'compliance/platform-test-relocation.json').write_text(json.dumps({'source': self.old + '/lib.rs'}))
        self.reject('historical-view')

    def test_receipt_tamper_rejected(self):
        active, records = self.apply()
        catalog = dict(schema=5, packages=active, renames=records, rename_records_sha256=naming.hashes(records))
        records[self.old]['reason'] = 'tampered'
        with self.assertRaisesRegex(ValueError, 'receipt drift'):
            naming.validate_history(self.root, catalog)

    def test_external_current_receipt_must_match(self):
        _, records = self.apply()
        with self.assertRaisesRegex(ValueError, 'current external owner'):
            naming.check_current_external(self.previous, {}, records)

    def test_remap_preserves_historical_sources(self):
        _, records = self.apply()
        remapped = naming.remap_previous(self.previous['external_tests'], records)
        self.assertEqual(remapped[self.new]['files'], self.owner['files'])
        self.assertEqual(remapped[self.new]['directory'], self.new)
        self.assertEqual(self.previous['external_tests'][self.old], self.owner)

    def test_exact_byte_count_required(self):
        self.item['destination_external_tests']['bytes'] += 1
        self.reject('byte count')

    def test_unknown_plan_fields_rejected(self):
        self.plan['allow_loss'] = True
        self.reject('invalid explicit')

    def test_unknown_record_fields_rejected(self):
        self.item['allow_loss'] = True
        self.reject('invalid explicit')

    def test_destination_name_must_match_directory(self):
        self.item['destination_name'] = 'different-name'
        self.reject('rename identity')

    def test_destination_cannot_replace_registered_source(self):
        self.previous['packages'][self.new] = deepcopy(self.package)
        self.reject('duplicate, reused')

    def test_destination_cannot_reuse_retired_source(self):
        self.previous['retired'] = {self.new: deepcopy(self.package)}
        self.reject('duplicate, reused')

    def test_wrong_origin_successor_rejected(self):
        active, records = self.apply()
        active[self.new]['origin_directory'] = 'another/origin'
        catalog = dict(schema=5, packages=active, renames=records, rename_records_sha256=naming.hashes(records))
        with self.assertRaisesRegex(ValueError, 'same-origin'):
            naming.validate_history(self.root, catalog)

    def test_external_relocation_outside_new_root_rejected(self):
        self.item['destination_external_tests']['files'][0]['path'] = 'test/other/test.rs'
        self.reject('historical source')

    def test_external_owner_cannot_disappear(self):
        self.item['destination_external_tests'] = None
        self.reject('discard an external')

    def add_second_fragment(self):
        row = deepcopy(self.owner['files'][0])
        row['path'] = self.test_old + '/second.rs'
        self.owner['files'].append(row)
        self.owner['file_count'] += 1
        self.owner['bytes'] *= 2
        self.item['external_tests'] = deepcopy(self.owner)
        current = self.item['destination_external_tests']
        row = deepcopy(current['files'][0])
        row['path'] = self.test_new + '/second.rs'
        current['files'].append(row)
        current['file_count'] += 1
        current['bytes'] *= 2
        (self.root / row['path']).write_bytes((self.root / self.test_new / 'test.rs').read_bytes())

    def test_multiple_fragments_of_one_source_preserved(self):
        self.add_second_fragment()
        _, records = self.apply()
        self.assertEqual(len(records[self.old]['external_tests']['files']), 2)

    def test_duplicate_destination_fragment_rejected(self):
        self.add_second_fragment()
        self.owner['files'][1]['path'] = self.owner['files'][0]['path']
        self.item['external_tests'] = deepcopy(self.owner)
        self.item['destination_external_tests']['files'][1]['path'] = self.item['destination_external_tests']['files'][0]['path']
        self.reject('lost external fragment')

    def test_same_source_conflicting_historical_hash_rejected(self):
        self.add_second_fragment()
        self.owner['files'][1]['source_sha256_before'] = 'a' * 64
        self.item['external_tests'] = deepcopy(self.owner)
        self.item['destination_external_tests']['files'][1]['source_sha256_before'] = 'a' * 64
        self.reject('external source identity')

    def completed_catalog(self):
        active, records = self.apply()
        return dict(schema=5, packages=active, renames=records, rename_records_sha256=naming.hashes(records))

    def test_history_rejects_old_source_root_return(self):
        catalog = self.completed_catalog()
        (self.root / self.old).mkdir()
        with self.assertRaisesRegex(ValueError, 'old renamed source root'):
            naming.validate_history(self.root, catalog)

    def test_history_rejects_old_test_root_return(self):
        catalog = self.completed_catalog()
        (self.root / self.test_old).mkdir()
        with self.assertRaisesRegex(ValueError, 'old external test root'):
            naming.validate_history(self.root, catalog)

    def test_history_rejects_current_catalog_name_drift(self):
        catalog = self.completed_catalog()
        catalog['packages'][self.new]['name'] = 'changed'
        with self.assertRaisesRegex(ValueError, 'name identity drift'):
            naming.validate_history(self.root, catalog)

    def test_history_rejects_manifest_name_drift(self):
        catalog = self.completed_catalog()
        path = self.root / self.new / 'Cargo.toml'
        path.write_text(path.read_text().replace('value-core', 'changed'))
        with self.assertRaisesRegex(ValueError, 'manifest name drift'):
            naming.validate_history(self.root, catalog)

    def test_history_does_not_freeze_local_code_version(self):
        catalog = self.completed_catalog()
        path = self.root / self.new / 'Cargo.toml'
        path.write_text(path.read_text().replace('1.0.0', '1.0.1'))
        catalog['packages'][self.new]['version'] = '1.0.1'
        naming.validate_history(self.root, catalog)

    def test_retired_successor_retains_prefix_without_live_manifest(self):
        catalog = self.completed_catalog()
        package = catalog['packages'].pop(self.new)
        catalog['retired'] = {self.new: package}
        (self.root / self.new / 'Cargo.toml').unlink()
        records = naming.validate_history(self.root, catalog)
        self.assertIn(self.old + '/', naming.source_prefixes(records, self.new, package['origin_directory']))

    def test_history_rejects_active_retired_collision(self):
        catalog = self.completed_catalog()
        catalog['retired'] = {self.new: deepcopy(catalog['packages'][self.new])}
        with self.assertRaisesRegex(ValueError, 'identity drift'):
            naming.validate_history(self.root, catalog)

    def test_history_rejects_duplicate_origin(self):
        catalog = self.completed_catalog()
        record = deepcopy(catalog['renames'][self.old])
        record['package']['directory'] = 'src/foundation/another-old'
        record['destination_directory'] = 'src/foundation/another-new'
        record['destination_name'] = 'another-new'
        catalog['renames']['src/foundation/another-old'] = record
        catalog['rename_records_sha256'] = naming.hashes(catalog['renames'])
        with self.assertRaisesRegex(ValueError, 'duplicate/missing historical rename origin'):
            naming.validate_history(self.root, catalog)

    def test_history_rejects_invalid_original_fragment_hash(self):
        catalog = self.completed_catalog()
        record = catalog['renames'][self.old]
        record['external_tests']['files'][0]['source_sha256_before'] = 'invalid'
        record['destination_external_tests']['files'][0]['source_sha256_before'] = 'invalid'
        catalog['rename_records_sha256'] = naming.hashes(catalog['renames'])
        with self.assertRaisesRegex(ValueError, 'historical rename external fragment'):
            naming.validate_history(self.root, catalog)

    def test_history_rejects_unrelated_historical_source_prefix(self):
        catalog = self.completed_catalog()
        record = catalog['renames'][self.old]
        for key in ('external_tests', 'destination_external_tests'):
            record[key]['files'][0]['source'] = 'src/foundation/unrelated/tests.rs'
        catalog['rename_records_sha256'] = naming.hashes(catalog['renames'])
        with self.assertRaisesRegex(ValueError, 'external source identity'):
            naming.validate_history(self.root, catalog)

    def test_rename_history_requires_schema_five(self):
        catalog = self.completed_catalog()
        catalog['schema'] = 4
        with self.assertRaisesRegex(ValueError, 'schema 5'):
            naming.validate_history(self.root, catalog)

    def test_schema_four_without_history_is_valid(self):
        self.previous['schema'] = 4
        self.assertEqual(naming.validate_history(self.root, self.previous), {})
        active, records = self.apply()
        self.assertEqual(set(active), {self.new})
        self.assertIn(self.old, records)

    def move_internal_file(self):
        old = self.root / self.new / 'lib.rs'
        new = self.root / self.new / 'module' / 'value.rs'
        new.parent.mkdir()
        old.rename(new)
        self.item['file_moves'] = {'lib.rs': 'module/value.rs'}
        self.item['files_sha256'] = inventory(self.root / self.new)

    def test_reviewed_internal_file_move(self):
        self.move_internal_file()
        active, records = self.apply()
        self.assertIn('module/value.rs', active[self.new]['files_sha256'])
        self.assertEqual(records[self.old]['package']['files_sha256'], self.package['files_sha256'])
        normalized = naming.remap_previous(self.previous['packages'], records)
        self.assertEqual(normalized[self.new]['files_sha256']['module/value.rs'], self.package['files_sha256']['lib.rs'])
        self.assertNotIn('lib.rs', normalized[self.new]['files_sha256'])
        self.assertEqual(naming.remap_previous(self.previous['external_tests'], records)[self.new]['files'], self.owner['files'])
        catalog = dict(schema=5, packages=active, renames=records, rename_records_sha256=naming.hashes(records))
        naming.validate_history(self.root, catalog)

    def test_internal_move_requires_explicit_map(self):
        self.move_internal_file()
        self.item.pop('file_moves')
        self.reject('bijective')

    def test_file_moves_cannot_overwrite_unmoved_file(self):
        self.item['file_moves'] = {'lib.rs': 'Cargo.toml'}
        self.reject('bijective')

    def test_file_moves_cannot_invent_source(self):
        self.item['file_moves'] = {'missing.rs': 'lib.rs'}
        self.reject('exact inventory paths')

    def test_file_moves_cannot_use_unknown_target(self):
        self.item['file_moves'] = {'lib.rs': '../escape.rs'}
        self.reject('exact inventory paths')

    def test_file_moves_reject_redundant_identity(self):
        self.item['file_moves'] = {'lib.rs': 'lib.rs'}
        self.reject('distinct exact')

    def test_file_moves_cannot_hide_content_drift(self):
        self.move_internal_file()
        (self.root / self.new / 'module/value.rs').write_text('unreviewed')
        self.reject('content differs')

    def test_file_moves_preserve_current_and_historical_counts(self):
        self.move_internal_file()
        self.item['files_sha256']['extra.rs'] = 'a' * 64
        self.reject('bijective')

    def test_file_moves_cannot_rewrite_history(self):
        self.move_internal_file()
        catalog = self.completed_catalog()
        catalog['renames'][self.old]['file_moves'] = {}
        with self.assertRaisesRegex(ValueError, 'receipt drift'):
            naming.validate_history(self.root, catalog)

    def test_bijective_swaps_are_complete_not_overwrites(self):
        before = {'a.rs': 'a' * 64, 'b.rs': 'b' * 64}
        after = {'a.rs': 'c' * 64, 'b.rs': 'd' * 64}
        self.assertEqual(naming.mapped_inventory(before, after, {'a.rs': 'b.rs', 'b.rs': 'a.rs'}),
                         {'b.rs': 'a' * 64, 'a.rs': 'b' * 64})

    def test_no_external_owner_supported(self):
        self.previous['external_tests'] = {}
        self.item['external_tests'] = None
        self.item['destination_external_tests'] = None
        active, records = self.apply()
        self.assertEqual(set(active), {self.new})
        naming.check_current_external(self.previous, {}, records)


if __name__ == '__main__':
    unittest.main()
