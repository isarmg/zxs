"""A real terminal consolidation proof must survive one live-root layout step."""
import base64
from copy import deepcopy
import gzip
import importlib
import os
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

HERE=Path(__file__).resolve().parent
sys.path.insert(0,str(HERE))
from test_source_layout import layout, encode_edits
from source_layout_fixture import digest, encoded, material

REPO=Path(os.environ.get('SOURCE_LAYOUT_REPOSITORY', str(HERE.parents[2] if len(HERE.parents)>=3 else Path.cwd())))
sys.path.append(str(REPO/'test/tools/rust-sources'))
sys.path.insert(0,str(REPO/'tools/rust-sources'))


class TerminalConsolidationLayout(unittest.TestCase):
    def setUp(self):
        module=patch.dict(sys.modules,{'source_layout':layout});module.start();self.addCleanup(module.stop)
        fixture=importlib.import_module('test_proof_consolidation')
        self.routing=importlib.import_module('proof_routing')
        self.projection=importlib.import_module('proof_consolidation')
        self.consolidation=importlib.import_module('source_consolidation')
        fixture.ProjectionFixture.setUp(self)
        # Give the pinned primary test a real source path whose layout rewrite
        # changes its physical length, while keeping all old receipts consistent.
        path='test/foundation/combined/mini.rs'
        before_test=b'include!("../../../src/foundation/combined/core.rs");\n'
        material(self.root,{path:before_test})
        input_owner=self.primary_input['external_tests']
        input_owner['bytes']=len(before_test)
        input_owner['files'][0].update(sha256=digest(before_test),source_sha256_before=digest(before_test))
        self.before_bytes['test/foundation/mini/test.rs']=before_test
        owner=self.record['destination_external_tests']
        owner['files'][0].update(sha256=digest(before_test),source_sha256_before=digest(before_test))
        owner['bytes']=sum((self.root/row['path']).stat().st_size for row in owner['files'])
        self.catalog['external_tests'][self.destination]=deepcopy(owner)
        selected=self.projection.projection(self.root,self.record)
        self.row.update(external_before=deepcopy(input_owner),external_after=deepcopy(selected['external']),
                        binding=dict(kind='consolidation',record=deepcopy(self.record)))
        self.row['files'][-1].update(before_sha256=digest(before_test),after_sha256=digest(before_test))
        records={self.consolidation.receipt_id(self.record):deepcopy(self.record)}
        self.catalog.update(consolidations=records,consolidation_records_sha256=self.consolidation.hashes(records))
        raw=gzip.compress(encoded(self.row),mtime=0);proof_id=digest(raw)
        material(self.root,{'compliance/proof-routing/'+proof_id+'.json.gz':raw})
        pins=patch.object(self.routing,'RECEIPTS',(proof_id,));pins.start();self.addCleanup(pins.stop)
        self.catalog['proof_routing_receipts']=[proof_id]
        self.previous_layout=deepcopy(self.catalog)
        self.old=self.destination;self.new='src/foundation/networking/combined'
        new_directory=self.root/self.new;new_directory.parent.mkdir(parents=True,exist_ok=True)
        (self.root/self.old).rename(new_directory)
        after_test=before_test.replace(b'src/foundation/combined/',b'src/foundation/networking/combined/')
        material(self.root,{path:after_test})
        self.current=deepcopy(self.previous_layout)
        package=self.current['packages'].pop(self.old);package['directory']=self.new
        self.current['packages'][self.new]=package
        for merged in self.current['merges']:merged.update(directory=self.new,merged_into=self.new)
        after_owner=self.current['external_tests'].pop(self.old)
        after_owner.update(directory=self.new,bytes=owner['bytes']+len(after_test)-len(before_test))
        after_owner['files'][0]['sha256']=digest(after_test)
        self.current['external_tests'][self.new]=after_owner
        frozen={str(p.relative_to(self.root)):digest(p.read_bytes()) for p in (self.root/'compliance').rglob('*') if p.is_file()}
        self.layout_receipt=dict(schema=1,reason='Relocate an already terminal pinned consolidated physical owner.',
            categories={'networking':'Networking'},predecessor_catalog_sha256=digest(encoded(self.previous_layout)),
            history_hashes={key:digest(encoded(self.previous_layout.get(key))) for key in layout.HISTORY_FIELDS},
            frozen_evidence=frozen,packages=[dict(directory=self.old,destination_directory=self.new,
                package=deepcopy(self.previous_layout['packages'][self.old]),
                merged_packages=deepcopy(self.previous_layout['merges']),
                files_sha256=deepcopy(package['files_sha256']),external_tests=deepcopy(owner),
                destination_external_tests=deepcopy(after_owner))],stationary_owners={},
            file_edits={path:dict(after_path=path,before_sha256=digest(before_test),after_sha256=digest(after_test),
                before_size=len(before_test),after_size=len(after_test),replacements=encode_edits(before_test,after_test))})
        raw=gzip.compress(encoded(self.layout_receipt),mtime=0);receipt_id=digest(raw)
        material(self.root,{layout.receipt_path(receipt_id):raw})
        pins=patch.object(layout,'PINNED_RECEIPTS',(receipt_id,));pins.start();self.addCleanup(pins.stop)
        self.current[layout.KEY]=receipt_id
        material(self.root,{'compliance/rust-sources.json':encoded(self.current)})
        self.view=layout.load(self.root,self.current)
        self.view.verify_installation(self.previous_layout,self.current,digest(encoded(self.previous_layout)))

    def test_semantic_maintenance_reverses_before_exact_layout_inverse(self):
        import proof_maintenance as maintenance
        import source_layout_corrections
        before_catalog = deepcopy(self.current)
        before = maintenance.state(before_catalog, self.new)
        path = 'test/foundation/combined/mini.rs'
        raw = (self.root / path).read_bytes()
        changed = raw.replace(b'include!', b'include_str!')
        material(self.root, {path: changed})
        owner = self.current['external_tests'][self.new]
        owner['files'][0]['sha256'] = digest(changed)
        owner['bytes'] += len(changed) - len(raw)
        record = dict(schema=1, reason='Preserve an error source after grouped layout',
                      terminal=self.current['proof_routing_receipts'][0], parent=None,
                      layout={key:self.current.get(key) for key in (layout.KEY, source_layout_corrections.KEY)},
                      before=before, after=maintenance.state(deepcopy(self.current), self.new),
                      files=[dict(before=path, after=path, before_sha256=digest(raw), after_sha256=digest(changed),
                                  replacements=[dict(old=base64.b64encode(b'include!').decode(),
                                                     new=base64.b64encode(b'include_str!').decode())])])
        compressed = gzip.compress(encoded(record), mtime=0); receipt = digest(compressed)
        material(self.root, {'compliance/proof-maintenance/' + receipt + '.json.gz':compressed})
        self.current[maintenance.KEY] = [receipt]
        with layout.activate(self.view), self.assertRaisesRegex(ValueError, 'layout live edited content drift'):
            layout.read(self.root, path)
        with patch.object(maintenance, 'RECEIPTS', (receipt,)):
            view = self.routed()
            self.assertEqual(view.read('test/foundation/mini/test.rs'), self.before_bytes['test/foundation/mini/test.rs'])
            with layout.activate(self.view):
                maintenance.validate_refresh(self.root, before_catalog, self.current)
        with self.assertRaises(ValueError):
            self.routed()

    def routed(self):
        with layout.activate(self.view):return self.routing.load(self.root,self.current)

    def test_terminal_primary_inverse_uses_live_grouped_files_and_logical_test_size(self):
        routed=self.routed()
        expected={path:raw for path,raw in self.before_bytes.items()
                  if path.startswith(self.primary+'/') or path.startswith('test/foundation/mini/')}
        self.assertEqual(routed.bytes,expected)
        self.assertNotIn(self.secondary+'/lib.rs',routed.bytes)
        with layout.activate(self.view):
            current=routed.current_package(self.primary)
            self.assertEqual(current['directory'],self.new)
            self.assertTrue((self.root/current['directory']/'Cargo.toml').is_file())
        self.assertNotEqual(self.current['external_tests'][self.new]['bytes'],self.previous_layout['external_tests'][self.old]['bytes'])

    def test_foreign_grouped_source_tamper_not_absorbed_or_hidden(self):
        material(self.root,{self.new+'/utils.rs':b'pub fn changed() {}\n'})
        with self.assertRaises(ValueError):self.routed()

    def test_foreign_grouped_source_missing_not_recovered_from_old_snapshot(self):
        (self.root/self.new/'utils.rs').unlink()
        with self.assertRaises(ValueError):self.routed()

    def test_primary_external_semantic_tamper_rejected_through_both_inverses(self):
        p=self.root/'test/foundation/combined/mini.rs'
        p.write_bytes(p.read_bytes().replace(b'core.rs',b'utils.rs'))
        with self.assertRaises(ValueError):self.routed()


if __name__=='__main__':unittest.main(verbosity=2)
