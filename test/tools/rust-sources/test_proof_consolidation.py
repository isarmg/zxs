"""Narrow primary projection never absorbs a foreign origin into pinned proofs."""
import base64
from copy import deepcopy
import hashlib
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

TOOLS = Path(__file__).resolve().parents[3] / 'tools/rust-sources'
sys.path.insert(0, str(TOOLS))
import proof_consolidation as projection
import proof_routing as routing
import source_consolidation


def digest(data):
    return hashlib.sha256(data).hexdigest()


class ProjectionFixture(unittest.TestCase):
    """Reusable root/record/row/catalog/before_bytes fixture for routing tests."""
    def set_family_proof(self, name, proof):
        relative = 'compliance/' + name + '-test-relocation.json'
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        data = json.dumps(proof, sort_keys=True).encode()
        path.write_bytes(data)
        self.family_pins[name] = (relative, digest(data))
        return path

    def setUp(self):
        import proof_maintenance
        maintenance = patch.object(proof_maintenance, 'RECEIPTS', ())
        maintenance.start(); self.addCleanup(maintenance.stop)
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.primary, self.secondary, self.destination = ('src/foundation/mini', 'src/foundation/utils', 'src/foundation/combined')
        self.before_bytes = {}
        inputs = []
        for directory, name, version in [(self.primary, 'mini', '1.0.0'), (self.secondary, 'utils', '2.0.0')]:
            manifest = ('[package]\nname = "' + name + '"\nversion = "' + version + '"\n').encode()
            code = ('pub fn ' + name + '() {}\n').encode()
            test = ('assert_eq!("' + name + '", "' + name + '");\n').encode()
            self.before_bytes[directory + '/Cargo.toml'] = manifest
            self.before_bytes[directory + '/lib.rs'] = code
            test_path = 'test/foundation/' + name + '/test.rs'
            self.before_bytes[test_path] = test
            package = dict(directory=directory, name=name, version=version,
                origin_directory='vendor/' + name, upstream=dict(name=name, version=version),
                license='MIT', license_materials=[], files_sha256={'Cargo.toml': digest(manifest), 'lib.rs': digest(code)})
            owner = dict(directory=directory, origin_directory=package['origin_directory'],
                roots=['test/foundation/' + name], files=[dict(source=directory + '/test.rs', path=test_path,
                    sha256=digest(test), source_sha256_before=digest(test))], file_count=1, bytes=len(test))
            inputs.append(dict(package=package, external_tests=owner))
        self.primary_input, self.secondary_input = inputs
        self.previous = dict(schema=8, packages={x['package']['directory']:deepcopy(x['package']) for x in inputs}, merges=[],
            external_tests={x['package']['directory']:deepcopy(x['external_tests']) for x in inputs},
            retired={}, superseded={}, renames={}, source_moves={}, consolidations={}, consolidation_records_sha256={})
        production = {'Cargo.toml': b'[package]\nname = "combined"\nversion = "1.0.0"\n',
                      'core.rs': self.before_bytes[self.primary + '/lib.rs'],
                      'utils.rs': self.before_bytes[self.secondary + '/lib.rs'], 'glue.rs': b'pub const GLUE: bool = true;\n'}
        for path, data in production.items():
            target = self.root / self.destination / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
        after_owner = dict(directory=self.destination, origin_directory='vendor/mini', roots=['test/foundation/combined'],
                           files=[], file_count=2, bytes=0)
        external_routes = {}
        for item in inputs:
            before = item['external_tests']['files'][0]
            path = 'test/foundation/combined/' + item['package']['name'] + '.rs'
            current = self.root / path
            current.parent.mkdir(parents=True, exist_ok=True)
            current.write_bytes(self.before_bytes[before['path']])
            after_owner['files'].append(dict(before, path=path, origin_directory=item['package']['origin_directory']))
            after_owner['bytes'] += current.stat().st_size
            external_routes[before['path']] = dict(destination=path, replacements=[])
        self.record = dict(primary_directory=self.primary, destination_directory=self.destination,
            destination_name='combined', reason='Integrate unpinned utilities without changing pinned ownership', inputs=inputs,
            destination_files_sha256={path:digest(data) for path,data in production.items()},
            file_routes={self.primary:{'Cargo.toml':{'destination':'Cargo.toml'}, 'lib.rs':{'destination':'core.rs'}},
                self.secondary:{'Cargo.toml':{'archive_base64':base64.b64encode(self.before_bytes[self.secondary + '/Cargo.toml']).decode()},
                                'lib.rs':{'destination':'utils.rs'}}},
            new_files={'glue.rs':digest(production['glue.rs'])},
            destination_external_tests=after_owner, external_file_routes=external_routes)
        self.family_pins = {}
        ProjectionFixture.set_family_proof(self, 'platform', dict(
            roots={'test/foundation/mini':{}}, files=[{'source':self.primary+'/lib.rs'}]))
        ProjectionFixture.set_family_proof(self, 'ring', dict(roots={}, files=[]))
        stub = patch.object(routing, 'FAMILIES', self.family_pins)
        stub.start()
        self.addCleanup(stub.stop)
        selected = projection.projection(self.root, self.record)
        self.row = dict(schema=1, parent=None, family='platform', reason='Final reviewed consolidation',
            before=deepcopy(inputs[0]['package']), after=deepcopy(selected['package']),
            external_before=deepcopy(inputs[0]['external_tests']), external_after=deepcopy(selected['external']),
            binding=dict(kind='consolidation', record=deepcopy(self.record)), files=[])
        for before, sha in inputs[0]['package']['files_sha256'].items():
            target = self.record['file_routes'][self.primary][before]['destination']
            edits = [] if before != 'Cargo.toml' else [dict(old=base64.b64encode(b'name = "mini"').decode(),
                                                          new=base64.b64encode(b'name = "combined"').decode())]
            self.row['files'].append(dict(before=self.primary + '/' + before, after=self.destination + '/' + target,
                before_sha256=sha, after_sha256=self.record['destination_files_sha256'][target], replacements=edits))
        before = inputs[0]['external_tests']['files'][0]
        after = selected['external']['files'][0]
        self.row['files'].append(dict(before=before['path'], after=after['path'], before_sha256=before['sha256'],
                                     after_sha256=after['sha256'], replacements=[]))
        primary = deepcopy(inputs[0]['package'])
        primary.update(directory=self.destination, name='combined', files_sha256=deepcopy(self.record['destination_files_sha256']))
        secondary = deepcopy(inputs[1]['package'])
        secondary.update(directory=self.destination, files_sha256=deepcopy(self.record['destination_files_sha256']), merged_into=self.destination)
        records = {source_consolidation.receipt_id(self.record):deepcopy(self.record)}
        self.catalog = dict(self.previous, packages={self.destination:primary}, merges=[secondary],
            external_tests={self.destination:deepcopy(after_owner)}, consolidations=records,
            consolidation_records_sha256=source_consolidation.hashes(records))


class ProofConsolidation(ProjectionFixture):
    def reject(self, pattern='.'):
        with self.assertRaisesRegex(ValueError, pattern):
            projection.projection(self.root, self.record)

    def test_primary_only_projection_and_exact_full_union(self):
        original = deepcopy(self.record)
        result = projection.projection(self.root, self.record)
        self.assertEqual(set(result), {'package','external','inventory','roots','foreign'})
        self.assertEqual(set(result['package']['files_sha256']), {'Cargo.toml','core.rs'})
        self.assertEqual(result['package']['origin_directory'], 'vendor/mini')
        self.assertEqual([row['origin_directory'] for row in result['external']['files']], ['vendor/mini'])
        self.assertEqual(result['external']['file_count'], 1)
        self.assertEqual(result['external']['bytes'], len(self.before_bytes['test/foundation/mini/test.rs']))
        self.assertEqual(set(result['foreign']), {self.destination + '/utils.rs', self.destination + '/glue.rs', 'test/foundation/combined/utils.rs'})
        actual = {path.relative_to(self.root).as_posix():digest(path.read_bytes())
                  for base in result['roots'] for path in (self.root/base).rglob('*') if path.is_file()}
        self.assertEqual(result['inventory'], actual)
        self.assertEqual(result['roots'], [self.destination, 'test/foundation/combined'])
        self.assertEqual(self.record, original)

    def test_registered_binding_and_prospective_binding(self):
        selected = projection.projection(self.root, self.record)
        self.assertEqual(projection.validate_binding(self.root,self.row,self.catalog), selected)
        self.assertEqual(projection.validate_binding(self.root,self.row,self.previous,prospective=True), selected)

    def test_batch_authenticates_shared_history_once_and_fresh_each_call(self):
        rows = (self.row, deepcopy(self.row))
        with patch.object(source_consolidation, 'validate_history', wraps=source_consolidation.validate_history) as check:
            for count in (1, 2):
                results = projection.validate_bindings(self.root, rows, self.catalog)
                self.assertEqual(len(results), 2)
                self.assertEqual(check.call_count, count)
        self.catalog['consolidation_records_sha256'] = {}
        with self.assertRaises(ValueError):
            projection.validate_bindings(self.root, rows, self.catalog)
        with self.assertRaises(ValueError):
            projection.validate_binding(self.root, self.row, self.catalog)

    def test_batch_rejects_tampered_second_binding(self):
        changed = deepcopy(self.row)
        changed['before']['origin_directory'] = 'vendor/foreign'
        with self.assertRaisesRegex(ValueError, 'projection mismatch'):
            projection.validate_bindings(self.root, (self.row, changed), self.catalog)

    def test_unregistered_binding_rejected(self):
        with self.assertRaisesRegex(ValueError,'registered immutable binding'):
            projection.validate_binding(self.root,self.row,self.previous)
        with self.assertRaisesRegex(ValueError,'registered current catalog'):
            projection.validate_binding(self.root,self.row)

    def test_binding_kind_fields_and_primary_snapshots_are_exact(self):
        for change in [lambda row:row['binding'].update(kind='rename'), lambda row:row['binding'].update(origin='vendor/mini'),
                       lambda row:row['before'].update(origin_directory='vendor/utils'),
                       lambda row:row['after']['files_sha256'].update({'utils.rs':self.record['destination_files_sha256']['utils.rs']}),
                       lambda row:row['external_after']['files'][0].pop('origin_directory')]:
            row = deepcopy(self.row); change(row)
            with self.subTest(change=change), self.assertRaisesRegex(ValueError,'binding|projection mismatch'):
                projection.validate_binding(self.root,row,self.catalog)

    def test_secondary_cannot_share_primary_manifest_destination(self):
        self.record['file_routes'][self.secondary]['Cargo.toml'] = {'destination':'Cargo.toml'}
        self.reject('overlap')

    def test_secondary_cannot_share_primary_source_destination(self):
        self.record['file_routes'][self.secondary]['lib.rs'] = {'destination':'core.rs'}
        self.record['new_files']['utils.rs'] = self.record['destination_files_sha256']['utils.rs']
        self.reject('overlap')

    def test_primary_routes_must_be_bijective(self):
        self.record['file_routes'][self.primary]['lib.rs'] = {'destination':'Cargo.toml'}
        self.record['new_files']['core.rs'] = self.record['destination_files_sha256']['core.rs']
        self.reject('bijection')

    def test_primary_manifest_cannot_be_archived(self):
        self.record['file_routes'][self.primary]['Cargo.toml'] = {'archive_base64':base64.b64encode(self.before_bytes[self.primary+'/Cargo.toml']).decode()}
        self.reject('only redundant')

    def test_primary_manifest_stays_active_manifest(self):
        self.record['file_routes'][self.primary]['Cargo.toml'] = {'destination':'core.rs'}
        self.record['file_routes'][self.primary]['lib.rs'] = {'destination':'Cargo.toml'}
        self.reject('manifest-rooted')

    def test_foreign_route_cannot_escape(self):
        self.record['file_routes'][self.secondary]['lib.rs'] = {'destination':'../escape.rs'}
        self.reject('unsafe')

    def test_new_glue_must_be_explicit_and_disjoint(self):
        self.record['new_files']['core.rs'] = self.record['destination_files_sha256']['core.rs']
        self.reject('disjoint')

    def test_unaccounted_destination_file_rejected(self):
        self.record['destination_files_sha256']['unaccounted.rs'] = 'a'*64
        self.reject('coverage')

    def test_invalid_hash_rejected(self):
        self.record['destination_files_sha256']['utils.rs'] = 'not a hash'
        self.reject('hash')

    def test_full_snapshot_hash_drift_does_not_match_binding(self):
        self.row['binding']['record']['destination_files_sha256']['core.rs'] = 'a'*64
        with self.assertRaisesRegex(ValueError,'projection mismatch'):
            projection.validate_binding(self.root,self.row,self.catalog)

    def test_foreign_test_origin_cannot_be_reassigned_to_primary(self):
        self.record['destination_external_tests']['files'][1]['origin_directory'] = 'vendor/mini'
        self.reject('historical attribution')

    def test_primary_test_origin_tag_cannot_disappear(self):
        self.record['destination_external_tests']['files'][0].pop('origin_directory')
        self.reject('historical attribution')

    def test_foreign_historical_source_hash_cannot_change(self):
        self.record['destination_external_tests']['files'][1]['source_sha256_before'] = 'b'*64
        self.reject('historical attribution')

    def test_missing_primary_external_file_rejected(self):
        (self.root/'test/foundation/combined/mini.rs').unlink()
        self.reject('missing active primary')

    def test_primary_external_symlink_rejected(self):
        path=self.root/'test/foundation/combined/mini.rs';path.unlink();path.symlink_to(self.root/self.destination/'core.rs')
        self.reject('symlink')

    def test_projected_test_byte_count_is_bounded(self):
        with patch.object(projection,'MAX_LIVE_BYTES',1):
            self.reject('byte bound')

    def test_combined_inventory_is_bounded(self):
        with patch.object(projection,'MAX_FILES',5):
            self.reject('inventory bound')

    def test_secondary_platform_proof_owner_rejected(self):
        self.set_family_proof('platform', {'roots':{'test/foundation/utils':{}},'files':[]})
        self.reject('pinned secondary')

    def test_secondary_platform_source_without_test_root_rejected(self):
        self.set_family_proof('platform', {'roots':{},'files':[{'source':self.secondary+'/lib.rs'}]})
        self.reject('pinned secondary')

    def test_missing_family_proof_is_not_unpinned_secondary_authorization(self):
        for family, (relative, _) in self.family_pins.items():
            with self.subTest(family=family):
                path=self.root/relative;data=path.read_bytes();path.unlink()
                self.reject('missing pinned projection family proof')
                path.write_bytes(data)

    def test_tampered_other_family_cannot_hide_pinned_secondary(self):
        self.row['family']='ring'
        self.set_family_proof('ring', {'roots':{'test/foundation/mini':{}},'files':[]})
        platform=self.set_family_proof('platform', {'roots':{'test/foundation/utils':{}},'files':[]})
        with self.assertRaisesRegex(ValueError,'pinned secondary'):
            projection.validate_binding(self.root,self.row,prospective=True)
        platform.write_text('{"roots": {}, "files": []}')
        with self.assertRaisesRegex(ValueError,'pinned projection family proof changed'):
            projection.validate_binding(self.root,self.row,prospective=True)
        platform.unlink()
        with self.assertRaisesRegex(ValueError,'missing pinned projection family proof'):
            projection.validate_binding(self.root,self.row,prospective=True)

    def test_other_family_hash_mismatch_is_rejected_before_parsing(self):
        path=self.root/self.family_pins['ring'][0]
        path.write_bytes(b'not json')
        self.reject('pinned projection family proof changed')

    def test_family_proof_symlink_is_rejected_even_with_matching_bytes(self):
        path=self.root/self.family_pins['ring'][0]
        target=path.with_name('copy.json');target.write_bytes(path.read_bytes())
        path.unlink();path.symlink_to(target)
        self.reject('symlink')

    def test_primary_archived_rust_source_is_rejected(self):
        data=b'pub fn old_primary_branch() {}\n'
        package=self.record['inputs'][0]['package']
        package['files_sha256']['src/legacy.rs']=digest(data)
        self.record['file_routes'][self.primary]['src/legacy.rs']=dict(kind='archived-source',sha256=digest(data),
            archive_base64=base64.b64encode(data).decode(),
            reason='Reviewed optional branch removal must never bypass the pinned primary bijection.')
        self.reject('pinned primary files cannot be archived')

    def test_explicit_secondary_source_archive_remains_history_not_primary(self):
        data=b'pub fn old_utility_branch() {}\n'
        package=self.record['inputs'][1]['package']
        package['files_sha256']['src/legacy.rs']=digest(data)
        self.record['file_routes'][self.secondary]['src/legacy.rs']=dict(kind='archived-source',sha256=digest(data),
            archive_base64=base64.b64encode(data).decode(),
            reason='Reviewed redundant optional utility branch is preserved byte-for-byte in its own origin receipt.')
        result=projection.projection(self.root,self.record)
        self.assertNotIn('src/legacy.rs',result['package']['files_sha256'])
        self.assertFalse(any(path.endswith('/legacy.rs') for path in result['inventory']))
        self.assertEqual(self.record['file_routes'][self.secondary]['src/legacy.rs']['sha256'],digest(data))

    def test_secondary_ring_owner_is_rejected_without_family_fallback(self):
        ring='src/foundation/ring'
        self.record['inputs'][1]['package']['directory']=ring
        self.record['file_routes'][ring]=self.record['file_routes'].pop(self.secondary)
        self.reject('pinned secondary')

    def test_family_proof_size_bound_is_enforced(self):
        with patch.object(projection,'MAX_PROOF_BYTES',5):
            self.reject('bounded projection family')

    def test_primary_platform_proof_is_not_secondary(self):
        projection.projection(self.root,self.record)

    def test_missing_current_merge_rejected(self):
        self.catalog['merges']=[]
        with self.assertRaisesRegex(ValueError,'origin coverage'):
            projection.validate_binding(self.root,self.row,self.catalog)

    def test_current_primary_foreign_inventory_cannot_shrink(self):
        del self.catalog['packages'][self.destination]['files_sha256']['utils.rs']
        with self.assertRaisesRegex(ValueError,'inventory drift'):
            projection.validate_binding(self.root,self.row,self.catalog)

    def test_current_merged_metadata_must_match_bound_snapshot(self):
        self.catalog['merges'][0]['description']='unbound metadata'
        with self.assertRaisesRegex(ValueError,'full merged owner'):
            projection.validate_binding(self.root,self.row,self.catalog)

    def test_current_external_full_owner_must_match(self):
        self.catalog['external_tests'][self.destination]['files'].pop()
        with self.assertRaisesRegex(ValueError,'full active owner'):
            projection.validate_binding(self.root,self.row,self.catalog)

    def test_proof_primary_production_route_cannot_be_permuted(self):
        row=deepcopy(self.row)
        row['files'][0]['after'],row['files'][1]['after']=row['files'][1]['after'],row['files'][0]['after']
        with self.assertRaisesRegex(ValueError,'exact bound primary route'):
            projection.validate_binding(self.root,row,self.catalog)

    def test_proof_primary_external_route_cannot_target_foreign_file(self):
        row=deepcopy(self.row)
        row['files'][-1]['after']='test/foundation/combined/utils.rs'
        with self.assertRaisesRegex(ValueError,'exact bound primary route'):
            projection.validate_binding(self.root,row,self.catalog)

    def test_proof_primary_route_inventory_cannot_shrink(self):
        row=deepcopy(self.row);row['files'].pop()
        with self.assertRaisesRegex(ValueError,'exact primary file routes'):
            projection.validate_binding(self.root,row,self.catalog)

    def test_registered_history_hash_tamper_rejected(self):
        key=next(iter(self.catalog['consolidation_records_sha256']))
        self.catalog['consolidation_records_sha256'][key]='0'*64
        with self.assertRaisesRegex(ValueError,'hash drift'):
            projection.validate_binding(self.root,self.row,self.catalog)

    def test_current_manifest_identity_checked(self):
        (self.root/self.destination/'Cargo.toml').write_text('[package]\nname="other"\nversion="1.0.0"\n')
        with self.assertRaisesRegex(ValueError,'manifest name/version'):
            projection.validate_binding(self.root,self.row,self.catalog)


if __name__ == '__main__':
    unittest.main()
