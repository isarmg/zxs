"""Adversarial tests for authenticated live-source layout using isolated fixtures.

SOURCE_LAYOUT_MODULE may select a separately staged module for review.
"""
import base64
from copy import deepcopy
import gzip
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from source_layout_fixture import Scenario, digest, encoded, material, move, patch_edits

DEFAULT_MODULE = ((HERE.parents[2] / 'tools/rust-sources/source_layout.py')
                  if len(HERE.parents) >= 3 else Path.cwd() / 'tools/rust-sources/source_layout.py')
MODULE = Path(os.environ.get('SOURCE_LAYOUT_MODULE', str(DEFAULT_MODULE)))
layout = sys.modules.get('source_layout')
if layout is None or Path(layout.__file__).resolve() != MODULE.resolve():
    SPEC = importlib.util.spec_from_file_location('source_layout', MODULE)
    layout = importlib.util.module_from_spec(SPEC)
    sys.modules['source_layout'] = layout
    SPEC.loader.exec_module(layout)


def encode_edits(before, after):
    return [dict(edit, before=base64.b64encode(edit['before'].encode()).decode(),
                 after=base64.b64encode(edit['after'].encode()).decode())
            for edit in patch_edits(before, after)]


def make_receipt(scenario):
    packages = []
    for old, new in scenario.routes.items():
        packages.append(dict(directory=old, destination_directory=new,
            package=deepcopy(scenario.previous['packages'][old]),
            merged_packages=deepcopy([p for p in scenario.previous['merges'] if p['directory']==old]),
            files_sha256=deepcopy(scenario.current['packages'][new]['files_sha256']),
            external_tests=deepcopy(scenario.previous['external_tests'].get(old)),
            destination_external_tests=deepcopy(scenario.current['external_tests'].get(new))))
    stationary = {'src/interface':dict(package=deepcopy(scenario.previous['packages']['src/interface']),
        merged_packages=[], files_sha256=deepcopy(scenario.current['packages']['src/interface']['files_sha256']))}
    edits = {}
    for path, before in scenario.before_bytes.items():
        after_path = move(path, scenario.routes); after = scenario.after_bytes[after_path]
        if after != before:
            edits[path] = dict(after_path=after_path, before_sha256=digest(before), after_sha256=digest(after),
                               before_size=len(before), after_size=len(after), replacements=encode_edits(before,after))
    return dict(schema=1, reason='Group exact active roots without changing source or origin identities.',
        categories={group:'Purpose of '+group for group in ('concurrency','platform','web-runtime')},
        predecessor_catalog_sha256=digest(encoded(scenario.previous)),
        history_hashes={field:digest(encoded(scenario.previous.get(field))) for field in layout.HISTORY_FIELDS},
        frozen_evidence=deepcopy(scenario.frozen), packages=packages, stationary_owners=stationary,
        file_edits=edits)


class LayoutFixture(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory(prefix='layout-adversarial-', dir='/tmp')
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.scenario = Scenario.create(self.root)
        self.receipt = make_receipt(self.scenario)
        pinned = patch.object(layout, 'PINNED_RECEIPTS', ())
        pinned.start(); self.addCleanup(pinned.stop)
        self.pin()

    def pin(self):
        raw = gzip.compress(encoded(self.receipt), mtime=0)
        self.receipt_id = digest(raw)
        path = self.root / layout.receipt_path(self.receipt_id)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(raw)
        layout.PINNED_RECEIPTS = (self.receipt_id,)
        self.scenario.current[layout.KEY] = self.receipt_id
        self.scenario.write_current()
        return self.receipt_id

    def load(self):
        return layout.load(self.root, self.scenario.current)

    def install(self):
        view = self.load()
        view.verify_installation(self.scenario.previous, self.scenario.current,
                                 digest(encoded(self.scenario.previous)))
        return view

    def alpha(self):
        return self.receipt['packages'][0]

    def add_unapproved_change(self, old, before, after):
        """Self-consistent forged hashes must not make a semantic edit a path edit."""
        new = move(old,self.scenario.routes)
        material(self.root,{new:after})
        self.receipt['file_edits'][old] = dict(after_path=new, before_sha256=digest(before), after_sha256=digest(after),
            before_size=len(before), after_size=len(after), replacements=encode_edits(before,after))
        directory = next(p for p in self.scenario.routes if old.startswith(p+'/'))
        relative = old[len(directory)+1:]
        next(row for row in self.receipt['packages'] if row['directory']==directory)['files_sha256'][relative] = digest(after)
        self.scenario.current['packages'][self.scenario.routes[directory]]['files_sha256'][relative] = digest(after)
        for owner in self.scenario.current['merges']:
            if owner['directory']==self.scenario.routes[directory]: owner['files_sha256'][relative]=digest(after)
        self.pin()


class ProjectionIsolation(LayoutFixture):
    def test_projection_matches_known_fixture_and_preserves_every_other_field(self):
        view = self.install()
        current = deepcopy(self.scenario.current)
        current['extra_metadata'] = {'nested': [1, {'value': 'retained'}]}
        original = deepcopy(current)
        expected = deepcopy(current)
        for key in ('packages', 'merges', 'external_tests'):
            expected[key] = deepcopy(self.scenario.previous[key])
        projected = view.project_catalog(current)
        self.assertEqual(encoded(projected), encoded(expected))
        self.assertEqual(list(projected), list(expected))
        self.assertEqual(current, original)

        projected['packages']['src/foundation/alpha']['files_sha256']['src/lib.rs'] = 'changed'
        self.assertEqual(projected['merges'][0]['files_sha256'],
                         expected['merges'][0]['files_sha256'])
        projected['merges'][0]['files_sha256'].clear()
        projected['external_tests']['src/foundation/alpha']['files'][0]['sha256'] = 'changed'
        projected['extra_metadata']['nested'][1]['value'] = 'changed'
        self.assertEqual(current, original)
        self.assertEqual(projected['packages']['src/foundation/alpha']['name'],
                         expected['packages']['src/foundation/alpha']['name'])
        self.assertEqual(view.project_catalog(current), expected)

    def test_untouched_aliases_stay_shared_but_rebuilt_inventories_are_separate(self):
        view = self.install()
        current = deepcopy(self.scenario.current)
        shared = {'values': [1]}
        current['unknown_a'] = shared
        current['unknown_b'] = shared
        current['unknown_inventory_alias'] = current['packages']
        projected = view.project_catalog(current)
        self.assertIs(projected['unknown_a'], projected['unknown_b'])
        self.assertIsNot(projected['unknown_a'], shared)
        self.assertEqual(projected['unknown_inventory_alias'], current['packages'])
        self.assertIsNot(projected['unknown_inventory_alias'], current['packages'])
        self.assertIsNot(projected['unknown_inventory_alias'], projected['packages'])
        projected['unknown_a']['values'].append(2)
        self.assertEqual(projected['unknown_b']['values'], [1, 2])
        self.assertEqual(shared, {'values': [1]})
        self.assertEqual(list(projected), list(current))

    def test_missing_inventory_fields_keep_empty_projection_and_independent_metadata(self):
        view = self.install()
        original = {'metadata': {'values': [1, 2]}}
        projected = view.project_catalog(original)
        self.assertEqual(projected, {'metadata': {'values': [1, 2]},
                                     'packages': {}, 'merges': [], 'external_tests': {}})
        projected['metadata']['values'].append(3)
        self.assertEqual(original, {'metadata': {'values': [1, 2]}})

    def test_package_without_hash_inventory_keeps_absence_and_owned_metadata(self):
        view = self.install()
        package = {'directory': self.scenario.routes['src/foundation/alpha'],
                   'metadata': {'values': [1]}}
        projected = view._project_package(package)
        self.assertEqual(projected, {'directory': 'src/foundation/alpha',
                                     'metadata': {'values': [1]}})
        projected['metadata']['values'].append(2)
        self.assertEqual(package['metadata'], {'values': [1]})


class HappyPaths(LayoutFixture):
    def test_exact_install_read_inventory_and_current_paths(self):
        view = self.install()
        for path,before in self.scenario.before_bytes.items():
            self.assertEqual(view.read(path),before,path)
        old='src/foundation/alpha'
        self.assertEqual(view.current_path(old+'/src'),self.scenario.routes[old]+'/src')
        self.assertEqual(view.inventory(old),self.scenario.full_inventory(old))
        self.assertEqual(view.historical_path(self.scenario.routes[old]+'/src/value.rs'),old+'/src/value.rs')

    def test_projection_preserves_external_logical_sizes_and_frozen_histories(self):
        view=self.install(); original=deepcopy(self.scenario.current)
        projected=view.project_catalog(self.scenario.current)
        for key in ('packages','merges','external_tests'):
            self.assertEqual(projected[key],self.scenario.previous[key],key)
        for key in layout.HISTORY_FIELDS:
            self.assertEqual(projected.get(key),self.scenario.current.get(key),key)
        path='test/foundation/alpha/tests/smoke.rs'
        old=self.scenario.previous['external_tests']['src/foundation/alpha']
        now=self.scenario.current['external_tests'][self.scenario.routes['src/foundation/alpha']]
        self.assertNotEqual(old['bytes'],now['bytes'])
        self.assertEqual(old['bytes'],len(view.read(path)))
        self.assertEqual(now['bytes'],(self.root/path).stat().st_size)
        self.assertEqual(original,self.scenario.current,'projection must not mutate its input')

    def test_exact_origin_bound_source_prefixes(self):
        view=self.install(); new=self.scenario.routes['src/foundation/alpha']
        self.assertEqual(view.source_prefixes(new,'vendor/alpha'),('src/foundation/alpha/',))
        self.assertEqual(view.source_prefixes(new,'vendor/not-alpha'),())
        self.assertEqual(view.source_prefixes('src/foundation/platform/alpha','vendor/alpha'),())

    def test_ordinary_implementation_evolves_without_frozen_snapshot_masking(self):
        self.install()
        old='src/foundation/plain/src/lib.rs'; new=move(old,self.scenario.routes)
        raw=b'pub fn identity(value: u8) -> u8 { value.saturating_add(1) }\n'
        material(self.root,{new:raw})
        current=self.scenario.current['packages'][self.scenario.routes['src/foundation/plain']]
        current['files_sha256']['src/lib.rs']=digest(raw)
        view=self.load(); projected=view.project_catalog(self.scenario.current)
        self.assertEqual(view.read(old),raw)
        self.assertEqual(projected['packages']['src/foundation/plain']['files_sha256']['src/lib.rs'],digest(raw))
        self.assertNotEqual(digest(view.read(old)),self.scenario.previous['packages']['src/foundation/plain']['files_sha256']['src/lib.rs'])

    def test_no_layout_identity_on_independent_old_fixture(self):
        with tempfile.TemporaryDirectory(dir='/tmp') as directory:
            self.assertIsNone(layout.load(Path(directory),{}))


class InventoryAndIdentityFailures(LayoutFixture):
    def test_missing_package_from_receipt_rejected(self):
        self.receipt['packages'].pop(1)
        self.receipt['categories'].pop('platform')
        self.pin()
        with self.assertRaises(ValueError):self.install()

    def test_duplicate_source_or_destination_rejected(self):
        self.receipt['packages'].append(deepcopy(self.alpha()));self.pin()
        with self.assertRaises(ValueError):self.load()

    def test_wrong_group_and_package_basename_rejected(self):
        self.alpha()['destination_directory']='src/foundation/platform/not-alpha';self.pin()
        with self.assertRaises(ValueError):self.load()

    def test_deeper_root_and_unsafe_paths_rejected(self):
        self.alpha()['destination_directory']='src/foundation/concurrency/extra/alpha';self.pin()
        with self.assertRaises(ValueError):self.load()

    def test_missing_shared_origin_rejected(self):
        self.scenario.current['merges'].clear()
        with self.assertRaises(ValueError):self.load()

    def test_duplicate_shared_origin_rejected(self):
        self.scenario.current['merges'].append(deepcopy(self.scenario.current['merges'][0]))
        with self.assertRaises(ValueError):self.load()

    def test_current_name_version_origin_upstream_license_mutations_rejected(self):
        original=deepcopy(self.scenario.current)
        for field,value in [('name','other'),('version','2.0.0'),('origin_directory','vendor/other'),
                            ('upstream',{'name':'other'}),('license','GPL-3.0')]:
            with self.subTest(field=field):
                self.scenario.current=deepcopy(original)
                self.scenario.current['packages'][self.scenario.routes['src/foundation/alpha']][field]=value
                with self.assertRaises(ValueError):self.load()

    def test_stationary_owner_identity_mutation_rejected(self):
        self.scenario.current['packages']['src/interface']['name']='substituted-interface'
        with self.assertRaises(ValueError):self.load()

    def test_wrong_predecessor_catalog_rejected(self):
        with self.assertRaises(ValueError):
            self.load().verify_installation(self.scenario.previous,self.scenario.current,'f'*64)

    def test_stale_package_snapshot_even_with_matching_files_rejected(self):
        old='src/foundation/alpha'
        self.scenario.previous['packages'][old]['changes']='A later review happened.'
        self.receipt['predecessor_catalog_sha256']=digest(encoded(self.scenario.previous));self.pin()
        with self.assertRaises(ValueError):self.install()

    def test_missing_mutated_extra_and_empty_production_files_rejected_at_install(self):
        relative=self.scenario.routes['src/foundation/plain']+'/src/lib.rs'
        path=self.root/relative; original=path.read_bytes(); extra=path.parent/'extra.rs'
        for mutation in ('missing','mutated','extra','empty'):
            with self.subTest(mutation=mutation):
                if mutation=='missing':path.unlink()
                elif mutation=='extra':extra.write_bytes(b'extra')
                else:path.write_bytes(b'' if mutation=='empty' else b'mutated')
                with self.assertRaises(ValueError):self.install()
                path.write_bytes(original);extra.unlink(missing_ok=True)

    def test_symlink_source_and_old_root_reappearance_rejected(self):
        path=self.root/(self.scenario.routes['src/foundation/plain']+'/src/lib.rs')
        old=path.read_bytes();path.unlink();path.symlink_to(self.root/'Cargo.lock')
        with self.assertRaises(ValueError):self.install()
        path.unlink();path.write_bytes(old)
        (self.root/'src/foundation/plain').mkdir()
        with self.assertRaises(ValueError):self.install()

    def test_old_root_reappearance_rejected_after_installation(self):
        self.install();(self.root/'src/foundation/plain').mkdir()
        with self.assertRaises(ValueError):self.load()

    def test_unknown_mapped_child_must_not_fall_back_to_live_extra(self):
        self.install();old='src/foundation/plain/src/unregistered.rs'
        material(self.root,{move(old,self.scenario.routes):b'unregistered'})
        with self.assertRaises(ValueError):self.load().read(old)

    def test_missing_live_file_never_uses_archive_or_stored_snapshot(self):
        view=self.install();old='src/foundation/plain/src/lib.rs'
        material(self.root,{'compliance/unrelated-archive.json':encoded({'files':{old:base64.b64encode(self.scenario.before_bytes[old]).decode()}})})
        (self.root/move(old,self.scenario.routes)).unlink()
        with self.assertRaises(ValueError):view.read(old)

    def test_trimmed_file_return_at_grouped_root_is_visible_and_rejected(self):
        view=self.install();old='src/foundation/trimmed/src/dead.rs'
        material(self.root,{move(old,self.scenario.routes):b'pub fn dead() {}\n'})
        self.scenario.current['packages'][self.scenario.routes['src/foundation/trimmed']]['files_sha256']['src/dead.rs']=digest(b'pub fn dead() {}\n')
        projected=view.project_catalog(self.scenario.current)
        self.assertIn('src/dead.rs',projected['packages']['src/foundation/trimmed']['files_sha256'])
        # The adapter must not let an old-root absence check conceal the new live file.
        self.assertTrue((self.root/view.current_path(old)).is_file())
        with self.assertRaises(ValueError):self.install()


class ReceiptAndExternalFailures(LayoutFixture):
    def test_frozen_evidence_tamper_rejected(self):
        target=self.root/'compliance/licenses/LICENSE-MIT';target.write_bytes(target.read_bytes()+b'changed')
        with self.assertRaises(ValueError):self.load()

    def test_historical_receipt_tamper_rejected(self):
        key=next(iter(self.scenario.current['source_trims']))
        self.scenario.current['source_trims'][key]['files'][0]['reason']='rewritten historical reason'
        with self.assertRaises(ValueError):self.load()

    def test_receipt_bytes_tamper_rejected(self):
        path=self.root/layout.receipt_path(self.receipt_id);path.write_bytes(path.read_bytes()+b'tamper')
        with self.assertRaises(ValueError):self.load()

    def test_unknown_pin_and_removed_catalog_binding_rejected(self):
        with patch.object(layout,'PINNED_RECEIPTS',()):
            with self.assertRaises(ValueError):self.load()
        del self.scenario.current[layout.KEY]
        with self.assertRaises(ValueError):self.load()

    def test_removed_receipt_rejected(self):
        (self.root/layout.receipt_path(self.receipt_id)).unlink()
        with self.assertRaises(ValueError):self.load()

    def test_compressed_expanded_and_live_file_bounds(self):
        for constant in ('MAX_COMPRESSED','MAX_EXPANDED'):
            with self.subTest(constant=constant),patch.object(layout,constant,1):
                with self.assertRaises(ValueError):self.load()
        view=self.install()
        with patch.object(layout,'MAX_FILE_BYTES',1):
            with self.assertRaises(ValueError):view.read('src/foundation/plain/src/lib.rs')

    def test_external_source_label_and_original_hash_rewrites_in_receipt_rejected(self):
        self.alpha()['destination_external_tests']['files'][0]['source']='src/foundation/alpha/tests/other.rs';self.pin()
        with self.assertRaises(ValueError):self.load()

    def test_current_external_source_label_rewrite_rejected(self):
        new=self.scenario.routes['src/foundation/alpha']
        self.scenario.current['external_tests'][new]['files'][0]['source']='src/foundation/alpha/tests/other.rs'
        with self.assertRaises(ValueError):self.load()

    def test_current_external_original_hash_and_fragment_drop_rejected(self):
        new=self.scenario.routes['src/foundation/alpha'];original=deepcopy(self.scenario.current)
        for mutation in ('hash','drop'):
            with self.subTest(mutation=mutation):
                self.scenario.current=deepcopy(original)
                owner=self.scenario.current['external_tests'][new]
                if mutation=='hash':owner['files'][0]['source_sha256_before']='e'*64
                else:owner['files'].clear();owner['file_count']=0;owner['bytes']=0
                with self.assertRaises(ValueError):self.load()

    def test_wrong_external_physical_size_rejected(self):
        self.alpha()['destination_external_tests']['bytes']+=1;self.pin()
        with self.assertRaises(ValueError):self.load()

    def test_missing_and_mutated_external_live_file_rejected(self):
        path=self.root/'test/foundation/alpha/tests/smoke.rs';old=path.read_bytes()
        for mutation in ('missing','changed'):
            with self.subTest(mutation=mutation):
                if mutation=='missing':path.unlink()
                else:path.write_bytes(old.replace(b'42',b'43'))
                with self.assertRaises(ValueError):self.install()
                path.write_bytes(old)

    def test_unattributed_external_adjacent_file_rejected_at_install(self):
        material(self.root,{'test/foundation/alpha/unattributed.rs':b'pub fn missing_receipt() {}\n'})
        with self.assertRaises(ValueError):self.install()


class PathEditFailures(LayoutFixture):
    def test_self_consistent_non_path_semantic_change_rejected(self):
        path='src/foundation/alpha/src/value.rs';before=self.scenario.before_bytes[path]
        self.add_unapproved_change(path,before,before.replace(b'42',b'43'))
        with self.assertRaises(ValueError):self.install()

    def test_self_consistent_manifest_feature_change_rejected(self):
        old='src/foundation/alpha/Cargo.toml'
        new=self.scenario.after_bytes[move(old,self.scenario.routes)].replace(b'default-features = false',b'default-features = true')
        self.add_unapproved_change(old,self.scenario.before_bytes[old],new)
        with self.assertRaises(ValueError):self.install()

    def test_unrecorded_routing_change_rejected(self):
        del self.receipt['file_edits']['src/foundation/alpha/src/lib.rs'];self.pin()
        with self.assertRaises(ValueError):self.load()

    def test_overlapping_out_of_bounds_and_boolean_offsets_rejected(self):
        key='src/foundation/alpha/Cargo.toml';original=deepcopy(self.receipt)
        for mutation in ('overlap','outside','boolean'):
            with self.subTest(mutation=mutation):
                self.receipt=deepcopy(original);edits=self.receipt['file_edits'][key]['replacements']
                if mutation=='overlap':edits.append(deepcopy(edits[0]))
                elif mutation=='outside':edits[0]['start']=10**9
                else:edits[0]['start']=True
                self.pin()
                with self.assertRaises(ValueError):self.load()

    def test_wrong_inverse_bytes_rejected(self):
        key='src/foundation/alpha/src/lib.rs'
        self.receipt['file_edits'][key]['replacements'][0]['after']=base64.b64encode(b'XX').decode();self.pin()
        with self.assertRaises(ValueError):self.install()

    def test_wrong_after_path_rejected(self):
        self.receipt['file_edits']['src/foundation/alpha/src/lib.rs']['after_path']='src/foundation/platform/plain/src/lib.rs';self.pin()
        with self.assertRaises(ValueError):self.load()

    def test_internal_document_path_is_not_rebased_out_of_package(self):
        path='src/foundation/alpha/src/lib.rs';raw=b'const DOC: &str = include_str!("../docs/internal.txt");\n'
        self.assertEqual(layout.rewrite(path,raw,self.scenario.routes),raw)

    def test_nearby_manifest_dir_expression_does_not_change_source_relative_include_base(self):
        path='src/foundation/alpha/src/lib.rs'
        raw=(b'const ROOT: &str = env!("CARGO_MANIFEST_DIR");\n'
             b'const DOC: &str = include_str!("../docs/internal.txt");\n')
        self.assertEqual(layout.rewrite(path,raw,self.scenario.routes),raw)

    def test_repository_document_path_rebases_without_content_change(self):
        path='src/foundation/alpha/src/lib.rs'
        self.assertEqual(layout.rewrite(path,self.scenario.before_bytes[path],self.scenario.routes),
                         self.scenario.after_bytes[move(path,self.scenario.routes)])

    def test_unrelated_string_and_numeric_code_unchanged(self):
        raw=b'pub const VALUE: u8 = 42;\nconst MESSAGE: &str = "no path here";\n'
        self.assertEqual(layout.rewrite('src/foundation/plain/src/lib.rs',raw,self.scenario.routes),raw)


class ContextFailures(LayoutFixture):
    def test_context_clears_on_success_and_exception(self):
        view=self.install()
        with layout.activate(view):self.assertIs(layout.load(self.root),view)
        self.assertIsNot(layout.load(self.root),view)
        with self.assertRaisesRegex(RuntimeError,'forced'):
            with layout.activate(view):raise RuntimeError('forced')
        self.assertIsNot(layout.load(self.root),view)

    def test_context_is_not_reused_for_other_root(self):
        view=self.install()
        with tempfile.TemporaryDirectory(dir='/tmp') as other:
            with layout.activate(view):self.assertIsNone(layout.load(Path(other),{}))

    def test_context_does_not_hide_changed_on_disk_catalog(self):
        view=self.install()
        with self.assertRaises(ValueError):
            with layout.activate(view):
                self.scenario.current['packages'][self.scenario.routes['src/foundation/plain']]['name']='intruder'
                self.scenario.write_current()
                with self.assertRaises(ValueError):layout.load(self.root)

    def test_scoped_helper_catalog_drift_is_rejected_on_exit(self):
        view=self.install()
        with self.assertRaises(ValueError):
            with layout.activate(view):
                self.scenario.current['packages'][self.scenario.routes['src/foundation/plain']]['name']='intruder'
                self.scenario.write_current()
                # Helpers may reuse the entry-authenticated state, but the operation
                # must not successfully return when its on-disk binding drifted.
                layout.current_path(self.root,'src/foundation/plain/src/lib.rs')
        self.assertIsNone(layout._ACTIVE.get())

    def test_stale_layout_cannot_enter_authenticated_operation(self):
        view=self.install()
        self.scenario.current['packages'][self.scenario.routes['src/foundation/plain']]['name']='intruder'
        self.scenario.write_current();entered=[]
        with self.assertRaises(ValueError):
            with layout.activate(view):entered.append(True)
        self.assertEqual(entered,[],'authentication must happen before operation body')
        self.assertIsNone(layout._ACTIVE.get())

    def test_live_path_edit_hash_is_checked_even_with_scoped_helpers(self):
        view=self.install();old='src/foundation/alpha/src/lib.rs'
        with layout.activate(view):
            path=self.root/move(old,self.scenario.routes)
            path.write_bytes(path.read_bytes()+b'// unreviewed semantic material\n')
            with self.assertRaises(ValueError):layout.read(self.root,old)

    def test_committed_catalog_requires_exact_expected_serialization(self):
        view=self.install();expected=encoded(self.scenario.current)
        with self.assertRaises(ValueError):
            with layout.activate(view):
                # Valid JSON and identical parsed identity do not suffice: an
                # output transaction must bind the exact validated bytes.
                material(self.root,{'compliance/rust-sources.json':expected+b'\n'})
                with self.assertRaises(ValueError):layout.written_catalog(self.root,expected)

    def test_exact_validated_catalog_commit_can_finish_operation(self):
        view=self.install();expected=encoded(self.scenario.current)+b'\n'
        with layout.activate(view):
            material(self.root,{'compliance/rust-sources.json':expected})
            layout.written_catalog(self.root,expected)
        self.assertEqual(view.disk_catalog_sha256,digest(expected))
        self.assertIsNone(layout._ACTIVE.get())

    def test_catalog_commit_cannot_authorize_identity_change(self):
        view=self.install()
        self.scenario.current['packages'][self.scenario.routes['src/foundation/plain']]['name']='intruder'
        expected=encoded(self.scenario.current)
        with self.assertRaises(ValueError):
            with layout.activate(view):
                material(self.root,{'compliance/rust-sources.json':expected})
                with self.assertRaises(ValueError):layout.written_catalog(self.root,expected)

    def test_one_thousand_helpers_hash_catalog_only_at_scope_boundaries(self):
        view=self.install();catalog=(self.root/'compliance/rust-sources.json').read_bytes()
        original=layout.digest;catalog_hashes=[]
        def counted(data):
            if data==catalog:catalog_hashes.append(True)
            return original(data)
        with patch.object(layout,'digest',side_effect=counted):
            with layout.activate(view):
                for _ in range(1000):
                    layout.current_path(self.root,'src/foundation/plain/src/lib.rs')
                    layout.read(self.root,'src/foundation/plain/src/lib.rs')
        self.assertEqual(len(catalog_hashes),2,'must hash at entry/exit, not each helper')

    def test_helper_immediately_detects_same_size_catalog_write_with_restored_mtime(self):
        view=self.install();path=self.root/'compliance/rust-sources.json';before=path.read_bytes();stat=path.stat()
        changed=before.replace(b'"name": "plain"',b'"name": "evill"',1)
        self.assertNotEqual(before,changed);self.assertEqual(len(before),len(changed))
        with layout.activate(view):
            try:
                path.write_bytes(changed);os.utime(path,ns=(stat.st_atime_ns,stat.st_mtime_ns))
                with self.assertRaises(ValueError):layout.current_path(self.root,'src/foundation/plain')
            finally:path.write_bytes(before)

    def test_helper_immediately_detects_catalog_inode_replacement(self):
        view=self.install();path=self.root/'compliance/rust-sources.json';before=path.read_bytes();stat=path.stat()
        changed=before.replace(b'"name": "plain"',b'"name": "evill"',1)
        with layout.activate(view):
            try:
                replacement=path.with_suffix('.replacement');replacement.write_bytes(changed)
                os.utime(replacement,ns=(stat.st_atime_ns,stat.st_mtime_ns));replacement.replace(path)
                with self.assertRaises(ValueError):layout.current_path(self.root,'src/foundation/plain')
            finally:path.write_bytes(before)

    def test_forced_exit_hash_catches_drift_even_with_unchanged_reported_stamp(self):
        view=self.install();path=self.root/'compliance/rust-sources.json';before=path.read_bytes()
        with self.assertRaises(ValueError):
            with patch.object(layout,'_stamp',return_value=view.disk_catalog_stamp):
                with layout.activate(view):
                    path.write_bytes(before+b' ')
                    layout.current_path(self.root,'src/foundation/plain')
        self.assertIsNone(layout._ACTIVE.get())

    def test_projected_catalog_sentinel_cannot_bypass_binding(self):
        self.scenario.current['_layout_projected']=True
        self.scenario.current[layout.KEY]='0'*64
        with self.assertRaises(ValueError):self.load()


if __name__=='__main__':unittest.main(verbosity=2)
