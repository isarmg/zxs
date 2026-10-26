"""Same-root maintenance must reconstruct every byte of the terminal union."""
import base64
from copy import deepcopy
import gzip
import json
import unittest
from unittest.mock import patch

import test_proof_consolidation as fixture
import proof_maintenance as maintenance
import proof_routing as routing
import source_layout
import source_layout_corrections


def enc(data):
    return base64.b64encode(data).decode()


class Maintenance(fixture.ProjectionFixture):
    def setUp(self):
        super().setUp()
        self.terminal = self.write_receipt('proof-routing', self.row)
        pin = patch.object(routing, 'RECEIPTS', (self.terminal,)); pin.start(); self.addCleanup(pin.stop)
        pin = patch.object(maintenance, 'RECEIPTS', ()); pin.start(); self.addCleanup(pin.stop)
        self.catalog['proof_routing_receipts'] = [self.terminal]
        self.previous = deepcopy(self.catalog)
        self.rows = []
        self.original = {path: routing.source_layout.read(self.root, path)
                         for path in fixture.projection.projection(self.root, self.record)['inventory']}

    def write_receipt(self, kind, row):
        raw = gzip.compress(json.dumps(row, sort_keys=True).encode(), mtime=0)
        sha = routing.digest(raw)
        path = self.root / 'compliance' / kind / (sha + '.json.gz')
        path.parent.mkdir(exist_ok=True); path.write_bytes(raw)
        return sha

    def pin(self):
        ids = [self.write_receipt('proof-maintenance', row) for row in self.rows]
        maintenance.RECEIPTS = tuple(ids)
        self.catalog[maintenance.KEY] = ids
        return ids

    def edit(self, old=b'mini', new=b'mini_v2', add_test=True):
        before = maintenance.state(deepcopy(self.catalog), self.destination)
        path = self.destination + '/core.rs'
        raw = (self.root / path).read_bytes(); changed = raw.replace(old, new)
        (self.root / path).write_bytes(changed)
        files = self.catalog['packages'][self.destination]['files_sha256']
        files['core.rs'] = routing.digest(changed)
        for package in self.catalog['merges']:
            package['files_sha256'] = deepcopy(files)
        owner = self.catalog['external_tests'][self.destination]
        if add_test:
            test = 'test/foundation/combined/regression.rs'; data = b'assert!(true);\n'
            (self.root / test).write_bytes(data)
            owner['files'].append(dict(source_kind='project-authored', source=test, path=test,
                                       sha256=routing.digest(data), source_sha256_before=routing.digest(data)))
            owner['file_count'] += 1; owner['bytes'] += len(data)
        row = dict(schema=1, layout={source_layout.KEY: None, source_layout_corrections.KEY: None}, reason='Retain blocking failure cause with a regression test', terminal=self.terminal,
                   parent=maintenance.RECEIPTS[-1] if self.rows else None, before=before,
                   after=maintenance.state(deepcopy(self.catalog), self.destination),
                   files=[dict(before=path, after=path, before_sha256=routing.digest(raw), after_sha256=routing.digest(changed),
                               replacements=[dict(old=enc(old), new=enc(new))])])
        self.rows.append(row); self.pin()
        return row

    def view(self):
        return routing.load(self.root, self.catalog)

    def test_exact_live_tip_and_terminal_bytes_and_refresh(self):
        self.edit()
        _, restored, _ = maintenance.restore(self.root, self.catalog)
        self.assertEqual(restored[self.destination]['bytes'], self.original)
        self.assertEqual(self.view().read(self.primary + '/lib.rs'), self.before_bytes[self.primary + '/lib.rs'])
        maintenance.validate_refresh(self.root, self.previous, self.catalog)
        routing.validate_refresh(self.root, self.previous, self.catalog)

    def test_second_maintenance_and_unrelated_owner_are_supported(self):
        self.edit(); previous = deepcopy(self.catalog)
        self.edit(b'mini_v2', b'mini_v3', add_test=False)
        unrelated = deepcopy(self.primary_input['package'])
        unrelated.update(name='unrelated', directory='src/foundation/unrelated', origin_directory='vendor/unrelated',
                         upstream=dict(name='unrelated', version='1.0.0'))
        files = {'Cargo.toml': b'[package]\nname = \"unrelated\"\nversion = \"1.0.0\"\n',
                 'lib.rs': b'pub fn unrelated() {}\n'}
        for path, data in files.items():
            target = self.root / unrelated['directory'] / path
            target.parent.mkdir(parents=True, exist_ok=True); target.write_bytes(data)
        unrelated['files_sha256'] = {path:routing.digest(data) for path,data in files.items()}
        self.catalog['packages'][unrelated['directory']] = unrelated
        maintenance.validate_refresh(self.root, previous, self.catalog)
        self.assertEqual(self.view().read(self.primary + '/lib.rs'), self.before_bytes[self.primary + '/lib.rs'])

    def test_without_receipt_and_plain_edit_after_terminal_still_rejected(self):
        self.edit(); maintenance.RECEIPTS = (); self.catalog.pop(maintenance.KEY)
        with self.assertRaisesRegex(ValueError, 'full active owner'):
            self.view()

    def test_receipt_raw_tamper_and_omitted_registration_rejected(self):
        self.edit(); sha = maintenance.RECEIPTS[0]
        path = self.root / 'compliance/proof-maintenance' / (sha + '.json.gz')
        raw = path.read_bytes(); path.write_bytes(raw + b'\n')
        with self.assertRaisesRegex(ValueError, 'immutable maintenance'):
            self.view()
        path.write_bytes(raw); self.catalog[maintenance.KEY] = []
        with self.assertRaisesRegex(ValueError, 'catalog history'):
            self.view()

    def test_reorder_omitted_step_and_wrong_parent_rejected(self):
        self.edit(); self.edit(b'mini_v2', b'mini_v3', add_test=False)
        ids = maintenance.RECEIPTS
        for wrong in (ids[::-1], ids[1:], ids + ids[:1]):
            maintenance.RECEIPTS = wrong; self.catalog[maintenance.KEY] = list(wrong)
            with self.subTest(wrong=wrong), self.assertRaises(ValueError):
                self.view()
        maintenance.RECEIPTS = ids
        self.rows[1]['parent'] = '0' * 64; self.pin()
        with self.assertRaisesRegex(ValueError, 'ancestry'):
            self.view()

    def test_live_newline_hash_drift_extra_missing_and_symlink_rejected(self):
        self.edit(); path = self.root / self.destination / 'core.rs'; raw = path.read_bytes()
        for data in (raw + b'\n', raw.replace(b'v2', b'v9')):
            path.write_bytes(data)
            with self.assertRaisesRegex(ValueError, 'content drift'):
                self.view()
        path.write_bytes(raw); extra = path.parent / 'extra.rs'; extra.write_bytes(b'x')
        with self.assertRaises(ValueError): self.view()
        extra.unlink(); path.unlink()
        with self.assertRaises(ValueError): self.view()
        path.symlink_to(path.parent / 'utils.rs')
        with self.assertRaises(ValueError): self.view()

    def test_omitted_hunk_wrong_hash_ambiguous_patch_and_extra_field_rejected(self):
        row = self.edit(); original = deepcopy(row)
        mutations = [lambda r: r['files'].clear(),
                     lambda r: r['files'][0]['replacements'].clear(),
                     lambda r: r['files'][0].update(before_sha256='0' * 64),
                     lambda r: r['files'][0]['replacements'][0].update(new=enc(b' ')),
                     lambda r: r.update(extra=True)]
        for mutate in mutations:
            row = deepcopy(original); mutate(row); self.rows = [row]; self.pin()
            with self.subTest(mutate=mutate), self.assertRaises(ValueError): self.view()

    def test_owner_identity_origin_and_external_bytes_drift_rejected(self):
        self.edit(); original = deepcopy(self.catalog)
        mutations = [lambda c: c['packages'][self.destination].update(version='2.0.0'),
                     lambda c: c['merges'][0].update(origin_directory='wrong'),
                     lambda c: c['external_tests'][self.destination].update(bytes=0)]
        for mutate in mutations:
            self.catalog = deepcopy(original); mutate(self.catalog)
            with self.subTest(mutate=mutate), self.assertRaises(ValueError): self.view()

    def test_new_upstream_attribution_and_changed_old_attribution_rejected(self):
        row = self.edit(); original = deepcopy(row)
        for target in (0, -1):
            row = deepcopy(original)
            row['after']['external']['files'][target]['source_sha256_before'] = '0' * 64
            self.rows = [row]; self.pin()
            with self.assertRaisesRegex(ValueError, 'attribution|project-authored'): self.view()

    def test_second_step_changes_added_and_historical_tests_without_losing_attribution(self):
        self.edit(); previous = deepcopy(self.catalog)
        before = maintenance.state(deepcopy(self.catalog), self.destination)
        patches = []
        owner = self.catalog['external_tests'][self.destination]
        for entry in (owner['files'][0], owner['files'][-1]):
            path = self.root / entry['path']; raw = path.read_bytes()
            old, new = (b'mini', b'minimum') if entry is owner['files'][0] else (b'true', b'1 == 1')
            # Existing upstream test contains its name twice; include full context.
            if entry is owner['files'][0]: old, new = b'assert_eq!("mini", "mini")', b'assert_eq!("minimum", "minimum")'
            changed = raw.replace(old, new); path.write_bytes(changed)
            entry['sha256'] = routing.digest(changed); owner['bytes'] += len(changed) - len(raw)
            patches.append(dict(before=entry['path'], after=entry['path'], before_sha256=routing.digest(raw),
                                after_sha256=entry['sha256'], replacements=[dict(old=enc(old), new=enc(new))]))
        row = dict(self.rows[-1], parent=maintenance.RECEIPTS[-1], before=before,
                   after=maintenance.state(deepcopy(self.catalog), self.destination), files=patches)
        self.rows.append(row); self.pin()
        maintenance.validate_refresh(self.root, previous, self.catalog)
        self.assertEqual(self.view().read('test/foundation/mini/test.rs'), self.before_bytes['test/foundation/mini/test.rs'])

    def test_bounded_stream_and_no_second_physical_read(self):
        self.edit()
        with patch.object(routing, 'MAX_LIVE_BYTES', 10):
            with self.assertRaisesRegex(ValueError, 'live material bound'): self.view()
        # Layout-independent maintenance must not call the read/hash inventory helper.
        with patch.object(source_layout, 'inventory', side_effect=AssertionError('second physical scan')):
            self.view()

    def test_previous_snapshot_cannot_skip_or_rewrite_owner(self):
        self.edit(); wrong = deepcopy(self.previous)
        wrong['packages'][self.destination]['files_sha256']['core.rs'] = '0' * 64
        with self.assertRaisesRegex(ValueError, 'previous catalog'): maintenance.validate_refresh(self.root, wrong, self.catalog)
        self.edit(b'mini_v2', b'mini_v3', add_test=False)
        with self.assertRaisesRegex(ValueError, 'multiple fresh'): maintenance.validate_refresh(self.root, self.previous, self.catalog)


if __name__ == '__main__':
    unittest.main()
