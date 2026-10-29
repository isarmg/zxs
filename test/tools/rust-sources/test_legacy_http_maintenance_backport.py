"""Schema 2 backport rows on the real HTTP maintenance chain (synthetic upstream fix).

The upstream base is the real hyper-1.12.0 `src/proto/h1/dispatch.rs`, rebuilt
from the extracted test slice and checked against the origin inventory; the
"fix" is a synthetic two-hunk edit, not an upstream or security change.
"""
from copy import deepcopy
import json
from pathlib import Path
import shutil
import sys
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(ROOT / 'tools/rust-sources'))
import proof_legacy_maintenance as legacy
import proof_routing as routing
import source_backport as backport
from test_legacy_http_maintenance import HttpChainFixture
from test_source_backport import recompute_replay

UPSTREAM = 'src/proto/h1/dispatch.rs'
PRODUCTION = legacy.OWNER + '/' + UPSTREAM
TESTS = 'test/foundation/http-transport/unit/src/proto/h1/dispatch/tests_770.rs'
CORRECTED = 'test/foundation/http-transport/adapters/unit/src/server/conn/auto/mod/guarded_tests_1135.rs'
UNROUTED = 'test/foundation/http-transport/unit/src/client/dispatch/tests_394.rs'
FUTURES = 'consolidation:eafb0b4185a7c210506e15a28d87995a7ff4898a240281ec013b11ec70a2acd3'


class HttpBackport(HttpChainFixture):
    def setUp(self):
        super().setUp()
        origins = self.root / backport.ORIGINS
        origins.parent.mkdir(parents=True)
        shutil.copyfile(ROOT / backport.ORIGINS, origins)
        self.layout.read = lambda path: (self.root / self.layout.current_path(path)).read_bytes()
        production, tests = (self.root / PRODUCTION).read_bytes(), (self.root / TESTS).read_bytes()
        head = b''.join(backport.split_lines(production)[:769]).replace(b'use futures::ready;\n', b'use futures_core::ready;\n', 1)
        self.base = head + tests
        origin = next(r for r in json.loads(origins.read_bytes())['packages']
                      if r['origin_directory'] == self.current['packages'][legacy.OWNER]['origin_directory'])
        self.assertEqual(backport.digest(self.base), origin['original_files_sha256'][UPSTREAM])
        self.fixed = self.base.replace(
            b'pub(crate) trait Dispatch {\n',
            b'pub(crate) trait Dispatch {\n    // synthetic fixture: futures_core::ready! is a crate path\n', 1).replace(
            b'    use std::time::Duration;\n\n',
            b'    use std::time::Duration;\n\n    #[test]\n    fn synthetic_backport_fixture() {}\n\n', 1)
        self.before = {PRODUCTION: production, TESTS: tests}
        self.backport_row = self.make_row()

    def make_row(self, fixed=None):
        package = self.current['packages'][legacy.OWNER]
        upstream = dict(base=dict(ref='v1.12.0', sha1=package['vcs']['git']['sha1']),
                        fix=dict(ref='synthetic', sha1='f' * 40, parent=package['vcs']['git']['sha1'],
                                 subject='synthetic fixture'), excluded=[])
        document, report, _ = backport.build_document(
            self.root, self.current, self.layout, legacy.OWNER, 'Synthetic schema 2 fixture; not an upstream fix.',
            upstream, {UPSTREAM: (self.base, fixed or self.fixed)},
            [{'from': 'futures_core', 'to': 'futures', 'basis': FUTURES}], [],
            dict(ids=[], disposition='hardening', basis='synthetic'), dict(gates=[], commands=[]))
        self.assertEqual(report, [])
        ours = {e['local_path']: (self.root / e['local_path']).read_bytes() for e in document['replay']['merged']}
        _, merged, _, _ = backport.merge_files(backport.upstream_material(document['upstream']), document['mapping'], ours, [])
        changes = {path: (ours[path], data) for path, data in merged.items() if data != ours[path]}
        return backport.build_receipt(document, self.current, changes, self.sha), changes

    def install(self, row=None, changes=None):
        row, changes = (row, changes) if row is not None else self.backport_row
        for path, (_, new) in changes.items():
            (self.root / path).write_bytes(new)
        self.current = backport.after_catalog(self.current, legacy.OWNER, changes)
        self.rows.append(row)
        self.register()
        return row

    def test_backport_row_is_accepted_and_inverted_for_production_and_tests(self):
        previous = deepcopy(self.current)
        row = self.install()
        self.assertEqual(sorted(f['after'] for f in row['files']), sorted([PRODUCTION, TESTS]))
        adapter = self.adapter()
        adapter.validate_refresh(previous, self.current)
        self.assertEqual(adapter.receipt_ids, legacy.RECEIPTS)
        before, after = adapter.step_bytes[legacy.RECEIPTS[-1]]
        self.assertEqual(before, self.before)
        self.assertIn(b'fn synthetic_backport_fixture() {}', after[TESTS])
        self.assertIn(b'// synthetic fixture: futures::ready!', after[PRODUCTION])
        live = (self.root / TESTS).read_bytes()
        self.assertEqual(adapter.read(TESTS, live), self.before[TESTS])
        self.assertEqual(adapter.project_hash(TESTS, routing.digest(live)), routing.digest(self.before[TESTS]))
        self.assertEqual(adapter.project_external(TESTS, routing.digest(live)),
                         (routing.digest(self.before[TESTS]), len(live) - len(self.before[TESTS])))
        logical = legacy.LOGICAL + '/' + UPSTREAM
        self.assertEqual(adapter.read(logical, (self.root / PRODUCTION).read_bytes()), self.before[PRODUCTION])
        self.assertEqual(adapter.read(UNROUTED, b'unrelated'), b'unrelated')
        with self.assertRaisesRegex(ValueError, 'live owner bytes'):
            adapter.read(TESTS, live + b'\n')
        prior = row['before']['external']
        self.assertEqual(row['after']['external']['bytes'] - prior['bytes'], len(live) - len(self.before[TESTS]))

    def test_tampered_live_test_bytes_fail(self):
        self.install()
        (self.root / TESTS).write_bytes((self.root / TESTS).read_bytes() + b'// drift\n')
        with self.assertRaisesRegex(ValueError, 'content drift'):
            self.adapter()

    def test_parent_discontinuity_is_rejected(self):
        row = self.install()
        row['parent'] = '0' * 64
        self.register()
        with self.assertRaisesRegex(ValueError, 'ancestry'):
            self.adapter()

    def edit_external(self, row, path, suffix=b'// synthetic external drift\n'):
        old = (self.root / path).read_bytes()
        new = old + suffix
        (self.root / path).write_bytes(new)
        for snapshot in (row['after']['external'], self.current['external_tests'][legacy.OWNER]):
            entry = next(e for e in snapshot['files'] if e['path'] == path)
            entry['sha256'] = routing.digest(new)
            snapshot['bytes'] += len(suffix)
        row['files'].append(dict(before=path, after=path, before_sha256=routing.digest(old), after_sha256=routing.digest(new),
                                 replacements=backport.reversible_replacements(old, new)))
        self.register()

    def test_external_change_outside_routed_upstream_files_is_rejected(self):
        row = self.install()
        self.edit_external(row, UNROUTED)
        with self.assertRaisesRegex(ValueError, 'outside the routed upstream files'):
            self.adapter()

    def test_corrected_external_test_cannot_be_backport_edited(self):
        row = self.install()
        self.edit_external(row, CORRECTED)
        with self.assertRaisesRegex(ValueError, 'external-path-corrected'):
            self.adapter()

    def test_production_file_add_and_manifest_change_are_rejected(self):
        row = self.install()
        original = deepcopy(row)
        for package in [row['after']['package'], *row['after']['merges']]:
            package['files_sha256']['src/new.rs'] = routing.digest(b'// new\n')
        self.register()
        with self.assertRaisesRegex(ValueError, 'inventory changed|file sets'):
            self.adapter()
        row.clear(); row.update(original)
        manifest = legacy.OWNER + '/Cargo.toml'
        old = (self.root / manifest).read_bytes()
        new = old.replace(b'name = "http-transport"', b'name = "forged"')
        (self.root / manifest).write_bytes(new)
        for snapshot in [row['after']['package'], *row['after']['merges'], self.current['packages'][legacy.OWNER],
                         *[m for m in self.current['merges'] if m['directory'] == legacy.OWNER]]:
            snapshot['files_sha256']['Cargo.toml'] = routing.digest(new)
        row['files'].append(dict(before=manifest, after=manifest, before_sha256=routing.digest(old),
                                 after_sha256=routing.digest(new), replacements=backport.reversible_replacements(old, new)))
        self.register()
        with self.assertRaisesRegex(ValueError, 'Rust source content'):
            self.adapter()

    def test_repinned_receipt_drift_names_the_input(self):
        row = self.install()
        original = deepcopy(row)
        tampered = self.base.replace(b'use futures_core::ready;', b'use futures_core::ready; ')
        cases = [
            ('result', lambda r: r['replay']['merged'][0].update(result_sha256='0' * 64), 'merge result drift'),
            ('identifiers', lambda r: r['mapping'].update(identifiers=[]), 'identifier mapping drift|line structure|local before'),
            ('identifier basis', lambda r: r['mapping']['identifiers'][0].update(basis='consolidation:' + '0' * 64), 'unregistered'),
            ('evidence', lambda r: r['mapping']['evidence'].pop(), 'evidence drift'),
            ('path', lambda r: r['mapping']['paths'][UPSTREAM].update(production=legacy.OWNER + '/src/lib.rs'), 'path mapping drift'),
            ('region', lambda r: r['mapping']['test_regions'][UPSTREAM][0].update(line_start=0), 'unresolved conflicts in ' + TESTS),
            ('base', lambda r: r['upstream']['files'][0].update(base=backport.pack(tampered), base_sha256=backport.digest(tampered)),
             'not the registered origin bytes'),
            ('advisory', lambda r: r['advisory'].update(ids=['CVE-2026-0001']), 'advisory'),
            ('license', lambda r: r['license'].update(materials=[]), 'license'),
        ]
        for label, mutate, message in cases:
            row.clear(); row.update(deepcopy(original)); mutate(row)
            self.register()
            with self.subTest(label=label), self.assertRaisesRegex(ValueError, message):
                self.adapter()

    def test_schema_1_successor_cannot_edit_routed_tests(self):
        row = self.install()
        row['schema'] = 1
        for key in backport.EXTENSION:
            row.pop(key)
        self.register()
        with self.assertRaisesRegex(ValueError, 'external tests'):
            self.adapter()

    def test_schema_1_successor_on_top_of_backport_and_refresh_window(self):
        before_backport = deepcopy(self.current)
        self.install()
        with self.assertRaisesRegex(ValueError, 'immediate predecessor'):
            legacy.load(self.layout, before_backport)
        legacy.load(self.layout, before_backport, prospective=True).validate_refresh(before_backport, self.current)
        previous = deepcopy(self.current)
        self.append_successor('src/lib.rs', b'//! # HTTP transport', b'//! # HTTP transport fixture')
        adapter = self.adapter()
        adapter.validate_refresh(previous, self.current)
        self.assertEqual(len(adapter.steps), 3)
        self.assertEqual(adapter.read(TESTS, (self.root / TESTS).read_bytes()), self.before[TESTS])

    def test_recomputed_region_and_resolutions_fail_normal_chain_authentication(self):
        row, _ = self.backport_row
        row['mapping']['test_regions'][UPSTREAM][0]['line_start'] -= 1
        results = recompute_replay(row, self.before)
        changes = {path: (self.before[path], data) for path, data in results.items() if data != self.before[path]}
        document = dict(owner=legacy.OWNER, reason=row['reason'],
                        **{key: row[key] for key in ('upstream', 'mapping', 'replay')},
                        advisory={key: row['advisory'][key] for key in ('ids', 'disposition', 'basis')},
                        consumer_tests={key: row['consumer_tests'][key] for key in ('gates', 'commands')})
        forged = backport.build_receipt(document, self.current, changes, self.sha)
        # The forged receipt is internally consistent and its hashes and
        # conflict resolutions have all been recomputed, not merely damaged.
        backport.check_replay(forged, self.before, results, self.current)
        backport.check_mapping(self.layout, self.current, forged)
        self.install(forged, changes)
        before = {path: (self.root / path).read_bytes() for path in self.before}
        with self.assertRaisesRegex(ValueError, 'test region drift'):
            self.adapter()
        self.assertEqual(before, {path: (self.root / path).read_bytes() for path in self.before})

    def test_normal_chain_region_authentication_never_reads_recursive_layout(self):
        self.install()
        with patch.object(self.layout, 'read', side_effect=AssertionError('recursive layout read')):
            adapter = self.adapter()
        self.assertEqual(adapter.step_bytes[legacy.RECEIPTS[-1]][0], self.before)


if __name__ == '__main__':
    unittest.main()
