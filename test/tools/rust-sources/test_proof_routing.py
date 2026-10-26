import base64
from copy import deepcopy
import gzip
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location('proof_routing', Path(__file__).resolve().parents[3] / 'tools/rust-sources/proof_routing.py')
routing = importlib.util.module_from_spec(SPEC); SPEC.loader.exec_module(routing)


def sha(data): return hashlib.sha256(data).hexdigest()
def enc(data): return base64.b64encode(data).decode()


class ActiveProofRouting(unittest.TestCase):
    def setUp(self):
        import proof_maintenance
        maintenance = patch.object(proof_maintenance, 'RECEIPTS', ())
        maintenance.start(); self.addCleanup(maintenance.stop)
        temp = tempfile.TemporaryDirectory(); self.addCleanup(temp.cleanup); self.root = Path(temp.name)
        self.old = 'src/foundation/entropy-1'; self.new = 'src/foundation/entropy'
        self.old_test = 'test/foundation/entropy-1'; self.new_test = 'test/foundation/entropy'
        old_manifest = b'[package]\nname = "entropy-old"\nversion = "1.0.0"\n'
        new_manifest = old_manifest.replace(b'entropy-old', b'entropy')
        self.old_bytes = {self.old+'/Cargo.toml': old_manifest, self.old+'/src/lib.rs': b'pub fn entropy() {}\n', self.old_test+'/tests.rs': b'assert!(true);\n'}
        self.new_bytes = {self.new+'/Cargo.toml': new_manifest, self.new+'/src/lib.rs': b'pub fn entropy() {}\n', self.new_test+'/tests.rs': b'assert!(true);\n'}
        for p,data in self.new_bytes.items():
            path=self.root/p; path.parent.mkdir(parents=True,exist_ok=True);path.write_bytes(data)
        common=dict(origin_directory='vendor/entropy',upstream=dict(name='entropy-old',version='1.0.0'),version='1.0.0')
        self.before=dict(common,directory=self.old,name='entropy-old',files_sha256={p[len(self.old)+1:]:sha(v) for p,v in self.old_bytes.items() if p.startswith(self.old+'/')})
        self.after=dict(common,directory=self.new,name='entropy',files_sha256={p[len(self.new)+1:]:sha(v) for p,v in self.new_bytes.items() if p.startswith(self.new+'/')})
        self.owner=dict(directory=self.old,origin_directory='vendor/entropy',roots=[self.old_test],file_count=1,bytes=len(self.old_bytes[self.old_test+'/tests.rs']),files=[dict(source=self.old+'/tests.rs',path=self.old_test+'/tests.rs',source_sha256_before=sha(b'assert!(true);\n'),sha256=sha(b'assert!(true);\n'))])
        self.next_owner=deepcopy(self.owner);self.next_owner.update(directory=self.new,roots=[self.new_test]);self.next_owner['files'][0]['path']=self.new_test+'/tests.rs'
        self.rename=dict(package=self.before,destination_directory=self.new,destination_name='entropy',reason='versionless role',files_sha256=self.after['files_sha256'],external_tests=self.owner,destination_external_tests=self.next_owner)
        self.row=dict(schema=1,parent=None,family='platform',reason='routing',before=self.before,after=self.after,external_before=self.owner,external_after=self.next_owner,binding=dict(kind='rename',record=self.rename),files=[])
        for a,b in zip(self.old_bytes,self.new_bytes):
            edits=[dict(old=enc(b'entropy-old'),new=enc(b'entropy'))] if a.endswith('Cargo.toml') else []
            self.row['files'].append(dict(before=a,after=b,before_sha256=sha(self.old_bytes[a]),after_sha256=sha(self.new_bytes[b]),replacements=edits))
        proof=self.root/'compliance/platform.json';proof.parent.mkdir();proof.write_text(json.dumps(dict(roots={self.old_test:{}})))
        stub=patch.object(routing,'FAMILIES',dict(platform=('compliance/platform.json',sha(proof.read_bytes()))));stub.start();self.addCleanup(stub.stop)
        self.previous=dict(packages={self.old:self.before},external_tests={self.old:self.owner})
        self.catalog=dict(schema=7,packages={self.new:self.after},external_tests={self.new:self.next_owner},renames={self.old:self.rename})
        self.pin()

    def pin(self, rows=None):
        rows=rows or [self.row];ids=[]
        for row in rows:
            data=gzip.compress(json.dumps(row,sort_keys=True).encode(),mtime=0);h=sha(data);ids.append(h)
            p=self.root/'compliance/proof-routing'/f'{h}.json.gz';p.parent.mkdir(exist_ok=True);p.write_bytes(data)
        stub=patch.object(routing,'RECEIPTS',tuple(ids));stub.start();self.addCleanup(stub.stop)
        self.catalog['proof_routing_receipts']=ids
        (self.root/'compliance/rust-sources.json').write_text(json.dumps(self.catalog))
        return ids

    def view(self):return routing.load(self.root,self.catalog)

    def test_exact_live_view_and_current_identity(self):
        v=self.view()
        self.assertEqual(v.read(self.old+'/Cargo.toml'),self.old_bytes[self.old+'/Cargo.toml'])
        self.assertEqual(v.inventory(self.old),{p:sha(b) for p,b in self.old_bytes.items() if p.startswith(self.old+'/')})
        self.assertEqual(v.current_path(self.old+'/src'),self.new+'/src')
        self.assertEqual(v.current_package(self.old)['name'],'entropy')
        self.assertEqual(v.baseline_path(self.new_test+'/tests.rs'),self.old_test+'/tests.rs')
        self.assertTrue(routing.authorize(self.root,self.previous,dict(self.rename,directory=self.old)))
        routing.validate_refresh(self.root,self.previous,self.catalog)

    def test_missing_changed_extra_and_empty_live_files_fail(self):
        p=self.root/(self.new+'/src/lib.rs');original=p.read_bytes()
        for mutation in ('missing','changed','extra','empty'):
            extra=p.parent/'extra.rs'
            if mutation=='missing':p.unlink()
            elif mutation=='extra':extra.write_text('extra')
            else:p.write_bytes(b'' if mutation=='empty' else b'changed')
            with self.assertRaises(ValueError):self.view()
            p.write_bytes(original);extra.unlink(missing_ok=True)

    def test_symlink_and_old_root_reappearance_fail(self):
        p=self.root/(self.new+'/src/lib.rs');p.unlink();p.symlink_to(self.root/(self.new+'/Cargo.toml'))
        with self.assertRaisesRegex(ValueError,'symlink'):self.view()
        p.unlink();p.write_bytes(self.new_bytes[self.new+'/src/lib.rs'])
        (self.root/self.old).mkdir()
        with self.assertRaisesRegex(ValueError,'old active proof root'):self.view()

    def test_duplicate_missing_or_wrong_hash_file_map_fails(self):
        original=deepcopy(self.row)
        for change in ('duplicate','missing','hash'):
            self.row=deepcopy(original)
            if change=='duplicate':self.row['files'].append(self.row['files'][0])
            elif change=='missing':self.row['files'].pop()
            else:self.row['files'][0]['before_sha256']='0'*64
            self.pin()
            with self.assertRaisesRegex(ValueError,'bijection|inventory'):self.view()

    def test_wrong_catalog_or_origin_or_binding_fails(self):
        self.catalog['packages'][self.new]=dict(self.after,name='other')
        with self.assertRaisesRegex(ValueError,'catalog owner'):self.view()
        self.catalog['packages'][self.new]=self.after
        self.row['binding']['record']=dict(self.rename,destination_name='other');self.pin()
        with self.assertRaisesRegex(ValueError,'binding'):self.view()

    def test_wrong_inverse_and_ambiguous_replacement_fail(self):
        self.row['files'][0]['replacements'][0]['old']=enc(b'wrong-name');self.pin()
        with self.assertRaisesRegex(ValueError,'historical content'):self.view()
        self.row['files'][0]['replacements'][0]['new']=enc(b'not-present');self.pin()
        with self.assertRaisesRegex(ValueError,'ambiguous'):self.view()

    def test_full_file_substitution_and_empty_current_fail(self):
        row=self.row['files'][0]
        row['replacements']=[dict(old=enc(self.old_bytes[row['before']]),new=enc(self.new_bytes[row['after']]))];self.pin()
        with self.assertRaisesRegex(ValueError,'full-file'):self.view()

    def test_receipt_tamper_truncation_bounds_and_escape_fail(self):
        h=self.catalog['proof_routing_receipts'][0];p=self.root/'compliance/proof-routing'/f'{h}.json.gz';original=p.read_bytes();p.write_bytes(original[:-3])
        with self.assertRaisesRegex(ValueError,'receipt changed'):self.view()
        p.write_bytes(original)
        for key in ('MAX_COMPRESSED','MAX_EXPANDED'):
            with patch.object(routing,key,1),self.assertRaisesRegex(ValueError,'bound'):self.view()
        self.row['files'][0]['after']='../escape';self.pin()
        with self.assertRaisesRegex(ValueError,'unsafe'):self.view()

    def test_chain_tip_removal_and_wrong_parent_fail(self):
        with patch.object(routing,'RECEIPTS',()),self.assertRaisesRegex(ValueError,'history was removed'):self.view()
        self.row['parent']='1'*64;self.pin()
        with self.assertRaisesRegex(ValueError,'parent'):self.view()

    def test_new_adapter_cannot_authorize_wrong_previous_or_other_plan(self):
        wrong=deepcopy(self.previous);wrong['packages'][self.old]['name']='wrong'
        with self.assertRaisesRegex(ValueError,'previous catalog'):routing.validate_refresh(self.root,wrong,self.catalog)
        with self.assertRaisesRegex(ValueError,'lacks exact'):routing.authorize(self.root,self.previous,dict(self.rename,directory=self.old,reason='different'))

    def test_active_adapter_refuses_retirement_or_unmapped_fallback(self):
        self.catalog['retired']={self.new:self.after}
        with self.assertRaisesRegex(ValueError,'retired archive'):self.view()
        self.catalog.pop('retired')
        with self.assertRaisesRegex(ValueError,'unmapped'):self.view().read(self.old+'/unknown.rs')

    def test_wrong_origin_version_and_old_external_root_fail(self):
        original=deepcopy(self.row)
        for field,value in [('origin_directory','other'),('version','2.0.0')]:
            self.row=deepcopy(original);self.row['after'][field]=value;self.pin()
            with self.assertRaises(ValueError):self.view()
        self.row=original;self.pin();(self.root/self.old_test).mkdir()
        with self.assertRaisesRegex(ValueError,'old active proof root'):self.view()

    def test_pinned_family_schema_and_patch_budget_fail_closed(self):
        with patch.object(routing,'MAX_LIVE_BYTES',1),self.assertRaisesRegex(ValueError,'bound'):self.view()
        with patch.object(routing,'MAX_PATCH_BYTES',1),self.assertRaisesRegex(ValueError,'bound'):self.view()
        with patch.object(routing,'MAX_REPLACEMENTS',0),self.assertRaisesRegex(ValueError,'bound'):self.view()
        self.catalog['schema']=6
        with self.assertRaisesRegex(ValueError,'schema 7'):self.view()
        self.catalog['schema']=7
        self.row['unexpected']=True;self.pin()
        with self.assertRaisesRegex(ValueError,'schema'):self.view()

    def test_two_step_edit_then_rename_replays_original(self):
        first=deepcopy(self.row);first['after']=deepcopy(self.before);first['after']['files_sha256']['src/lib.rs']=sha(b'pub fn entropy() { /* route */ }\n')
        first.update(external_after=self.owner,binding=dict(kind='edit',record=None))
        for file in first['files']:
            file['after']=file['before'];file['after_sha256']=file['before_sha256'];file['replacements']=[]
            if file['before'].endswith('lib.rs'):
                file.update(after_sha256=first['after']['files_sha256']['src/lib.rs'],replacements=[dict(old=enc(b'{}'),new=enc(b'{ /* route */ }'))])
        first_id=sha(gzip.compress(json.dumps(first,sort_keys=True).encode(),mtime=0))
        self.row['parent']=first_id;self.row['before']=first['after'];self.rename['package']=first['after']
        self.after['files_sha256']['src/lib.rs']=first['after']['files_sha256']['src/lib.rs']
        for file in self.row['files']:
            if file['before'].endswith('lib.rs'):file['before_sha256']=file['after_sha256']=first['after']['files_sha256']['src/lib.rs']
        (self.root/(self.new+'/src/lib.rs')).write_bytes(b'pub fn entropy() { /* route */ }\n')
        self.pin([first,self.row]);self.assertEqual(self.view().read(self.old+'/src/lib.rs'),b'pub fn entropy() {}\n')


if __name__=='__main__':unittest.main()
