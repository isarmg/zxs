"""Backport loop mechanics on synthetic owners; the real HTTP chain is covered separately."""
import base64
from contextlib import redirect_stdout
from copy import deepcopy
import gzip
import io
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'tools/rust-sources'))
import proof_legacy_maintenance as legacy
import proof_maintenance
import proof_routing
import source_backport as backport
import source_consolidation

OWNER = legacy.OWNER
ORIGIN = 'vendor/rust/web-1.0.0'
OLD = 'src/foundation/web'
TEST = 'test/foundation/web/unit/src/lib/tests_11.rs'
HELPER = 'src/foundation/futures-core'

BASE = b'''use futures_core::ready;

pub fn alpha() -> u32 {
    1
}

pub fn beta() -> u32 {
    2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_is_one() {
        assert_eq!(alpha(), 1);
    }
}
'''
FIXED = BASE.replace(b'    2\n}', b'    let _ = futures_core::task::Poll::Ready(());\n    3\n}').replace(
    b'        assert_eq!(alpha(), 1);\n    }\n', b'        assert_eq!(alpha(), 1);\n    }\n\n'
    b'    #[test]\n    fn beta_is_three() {\n        assert_eq!(beta(), 3);\n    }\n')
REGION = BASE[BASE.index(b'mod tests'):]
LOCAL = BASE[:BASE.index(b'mod tests')].replace(b'futures_core', b'futures').replace(
    b'    1\n}', b'    1 // local\n}') + b'include!("../../../../test/foundation/web/unit/src/lib/tests_11.rs");\n'


def enc(data):
    return base64.b64encode(data).decode()


def git(cwd, *args):
    return subprocess.run(['git', '-C', str(cwd), '-c', 'user.name=t', '-c', 'user.email=t@t.invalid', *args],
                          check=True, capture_output=True).stdout.decode().strip()


def consolidation_record():
    return dict(destination_directory=OWNER, destination_name='http-transport', primary_directory=OLD,
                inputs=[dict(package=dict(directory=OLD, name='web', origin_directory=ORIGIN)),
                        dict(package=dict(directory=HELPER, name='futures-core', origin_directory='vendor/rust/fc'))],
                file_routes={OLD: {'src/lib.rs': dict(destination='src/lib.rs'),
                                   'Cargo.toml': dict(destination='Cargo.toml')}},
                input_consolidations={})


def futures_record():
    return dict(destination_directory='src/foundation/futures', destination_name='futures',
                primary_directory='src/foundation/futures', input_consolidations={},
                inputs=[dict(package=dict(directory='src/foundation/futures', name='futures', origin_directory='vendor/f')),
                        dict(package=dict(directory=HELPER, name='futures-core', origin_directory='vendor/fc'))],
                file_routes={})


def recompute_replay(document, ours):
    """Forge a self-consistent replay, including resolutions bound to new conflicts.

    Used to distinguish authenticated routes from merely self-consistent plans
    and receipts. Choosing ours for every conflict is deliberate test input.
    """
    material = backport.upstream_material(document['upstream'])
    _, _, _, report = backport.merge_files(material, document['mapping'], ours, [])
    resolutions = [dict(file=item['file'], hunk_index=conflict['index'],
                        conflict_sha256=conflict['sha256'], resolution_base64=conflict['ours']['text'])
                   for item in report for conflict in item['conflicts']]
    entries, results, replacements, report = backport.merge_files(material, document['mapping'], ours, resolutions)
    assert all(conflict['resolved'] for item in report for conflict in item['conflicts'])
    document['replay'] = dict(merged=entries, identifier_replacements=replacements)
    return results


class Synthetic(unittest.TestCase):
    """A consolidated owner with one extracted upstream test slice and a local clone."""

    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='source-backport-')
        self.addCleanup(temporary.cleanup)
        self.top = Path(temporary.name)
        self.root = self.top / 'repo'
        self.clone = self.top / 'upstream'
        self.out = self.top / 'out'
        self.clone.mkdir()
        git(self.clone, 'init', '-q')
        (self.clone / 'src').mkdir()
        (self.clone / 'src/lib.rs').write_bytes(BASE)
        git(self.clone, 'add', '.'); git(self.clone, 'commit', '-qm', 'base'); git(self.clone, 'tag', 'v1.0.0')
        self.base_sha = git(self.clone, 'rev-parse', 'HEAD')
        (self.clone / 'src/lib.rs').write_bytes(FIXED)
        (self.clone / 'NOTES.md').write_text('unrelated\n')
        git(self.clone, 'add', '.'); git(self.clone, 'commit', '-qm', 'fix(beta): return three (#1)')
        self.fix_sha = git(self.clone, 'rev-parse', 'HEAD')
        self.files = {OWNER + '/src/lib.rs': LOCAL, OWNER + '/Cargo.toml': b'[package]\nname = "http-transport"\n',
                      TEST: REGION}
        for path, data in self.files.items():
            (self.root / path).parent.mkdir(parents=True, exist_ok=True)
            (self.root / path).write_bytes(data)
        record, futures = consolidation_record(), futures_record()
        self.rid, self.fid = source_consolidation.receipt_id(record), source_consolidation.receipt_id(futures)
        package = dict(directory=OWNER, name='http-transport', version='1.0.0', origin_directory=ORIGIN,
                       upstream=dict(name='web', version='1.0.0'), repository='https://example.invalid/web',
                       vcs=dict(git=dict(sha1=self.base_sha)), license_materials=['compliance/licenses/web/LICENSE'],
                       files_sha256={p[len(OWNER) + 1:]: backport.digest(d) for p, d in self.files.items() if p.startswith(OWNER)})
        entry = dict(path=TEST, source=OLD + '/src/lib.rs', source_sha256_before=backport.digest(BASE),
                     sha256=backport.digest(REGION), origin_directory=ORIGIN)
        self.catalog = dict(packages={OWNER: package}, merges=[],
                            consolidations={self.rid: record, self.fid: futures},
                            consolidation_records_sha256={self.rid: self.rid, self.fid: self.fid},
                            external_tests={OWNER: dict(directory=OWNER, origin_directory=ORIGIN, roots=['test/foundation/web'],
                                                        files=[entry], file_count=1, bytes=len(REGION))})
        self.write_catalog()
        origins = dict(packages=[dict(origin_directory=ORIGIN, original_files_sha256={
            'src/lib.rs': backport.digest(BASE), 'Cargo.toml': '0' * 64})])
        (self.root / backport.ORIGINS).parent.mkdir(parents=True)
        (self.root / backport.ORIGINS).write_text(json.dumps(origins))
        mapping = dict(schema_version=1, owners=[dict(directory=OWNER, roots=['test/foundation/web'],
                                                      files=[{k: v for k, v in entry.items() if k != 'origin_directory'}])])
        (self.root / backport.MAPPINGS).mkdir(parents=True)
        (self.root / backport.MAPPINGS / 'foundation.json').write_text(
            json.dumps(mapping, indent=2, sort_keys=True, ensure_ascii=False) + '\n')
        self.identifiers = [dict(to='futures', basis='consolidation:' + self.fid, **{'from': 'futures_core'})]
        pin = patch.object(legacy, 'RECEIPTS', ()); pin.start(); self.addCleanup(pin.stop)

    def write_catalog(self):
        (self.root / backport.CATALOG).parent.mkdir(parents=True, exist_ok=True)
        (self.root / backport.CATALOG).write_text(json.dumps(self.catalog, indent=2, sort_keys=True))

    def request(self, **extra):
        request = dict(owner=OWNER, upstream_clone=str(self.clone),
                       upstream=dict(base='v1.0.0', fix=self.fix_sha, files=['src/lib.rs']),
                       identifier_map=self.identifiers, reason='Synthetic backport for tool tests.',
                       advisory=dict(ids=[], disposition='hardening', basis='synthetic #1'),
                       consumer_tests=dict(gates=[], commands=[]))
        request.update(extra)
        return request

    def plan(self, **extra):
        with redirect_stdout(io.StringIO()):
            return backport.plan(self.root, self.request(**extra), self.out)


class Merge3(unittest.TestCase):
    def test_one_sided_identical_and_disjoint_changes_merge(self):
        base = b'a\nb\nc\nd\ne\n'
        merged, conflicts = backport.merge3(base, b'A\nb\nc\nd\ne\n', b'a\nb\nc\nd\nE\n')
        self.assertEqual((b''.join(merged), conflicts), (b'A\nb\nc\nd\nE\n', []))
        merged, conflicts = backport.merge3(base, b'a\nB\nc\nd\ne\n', b'a\nB\nc\nd\ne\n')
        self.assertEqual((b''.join(merged), conflicts), (b'a\nB\nc\nd\ne\n', []))
        merged, _ = backport.merge3(base, base, b'a\nb\nx\ny\nc\nd\ne\n')
        self.assertEqual(b''.join(merged), b'a\nb\nx\ny\nc\nd\ne\n')
        merged, _ = backport.merge3(base, b'a\nc\nd\ne\n', base)
        self.assertEqual(b''.join(merged), b'a\nc\nd\ne\n')

    def test_overlapping_and_adjacent_changes_conflict_with_exact_ranges(self):
        base = b'a\nb\nc\nd\n'
        merged, conflicts = backport.merge3(base, b'a\nB1\nc\nd\n', b'a\nB2\nc\nd\n')
        self.assertEqual(len(conflicts), 1)
        conflict = conflicts[0]
        self.assertEqual((conflict['base']['start'], conflict['base']['end']), (1, 2))
        self.assertEqual([base64.b64decode(conflict[k]['text']) for k in ('base', 'ours', 'theirs')],
                         [b'b\n', b'B1\n', b'B2\n'])
        self.assertEqual(b''.join(merged), b'a\n<<<<<<< ours\nB1\n||||||| base\nb\n=======\nB2\n>>>>>>> theirs\nc\nd\n')
        # Adjacent edits with no common line between them are one conflict, not two silent merges.
        _, conflicts = backport.merge3(base, b'a\nB\nc\nd\n', b'a\nb\nC\nd\n')
        self.assertEqual(len(conflicts), 1)
        # Two different insertions at the same point also conflict.
        _, conflicts = backport.merge3(base, b'a\nx\nb\nc\nd\n', b'a\ny\nb\nc\nd\n')
        self.assertEqual(len(conflicts), 1)

    def test_resolution_replaces_exactly_the_conflict_block(self):
        merged, conflicts = backport.merge3(b'a\nb\nc\n', b'a\nX\nc\n', b'a\nY\nc\n')
        self.assertEqual(backport.resolve(merged, conflicts, {0: b'XY\n'}), b'a\nXY\nc\n')
        with self.assertRaisesRegex(ValueError, 'cover exactly'):
            backport.resolve(merged, conflicts, {})

    def test_crlf_lone_cr_and_missing_final_newline_are_preserved(self):
        base = b'a\r\nb\rstill b\r\nc'
        merged, conflicts = backport.merge3(base, base.replace(b'a\r\n', b'A\r\n'), base + b'\r\nd')
        self.assertEqual(conflicts, [])
        self.assertEqual(b''.join(merged), b'A\r\nb\rstill b\r\nc\r\nd')
        self.assertEqual(backport.split_lines(b'x\ry\n'), [b'x\ry\n'])

    def test_merge_is_deterministic(self):
        base = b''.join(b'line %d\n' % i for i in range(300))
        ours = base.replace(b'line 10\n', b'ours 10\n').replace(b'line 200\n', b'')
        theirs = base.replace(b'line 100\n', b'theirs 100\nline 100\n').replace(b'line 10\n', b'theirs 10\n')
        runs = {backport.digest(b''.join(backport.merge3(base, ours, theirs)[0])) for _ in range(5)}
        self.assertEqual(len(runs), 1)
        reports = {json.dumps(backport.merge3(base, ours, theirs)[1], sort_keys=True) for _ in range(5)}
        self.assertEqual(len(reports), 1)


class Identifiers(unittest.TestCase):
    MAP = [{'from': 'futures_core', 'to': 'futures', 'basis': 'unused'}]

    def mapped(self, text):
        return backport.map_identifiers(text.encode(), self.MAP)

    def test_crate_roots_are_rewritten_and_recorded(self):
        for text, expected in (('use futures_core::ready;\n', 'use futures::ready;\n'),
                               ('    futures_core::ready!(x)\n', '    futures::ready!(x)\n'),
                               ('use futures_core;\n', 'use futures;\n'),
                               ('extern crate futures_core as fc;\n', 'extern crate futures as fc;\n'),
                               ('let s = ::futures_core::stream::empty();\n', 'let s = ::futures::stream::empty();\n'),
                               ('fn f<S: futures_core :: Stream>() {}\n', 'fn f<S: futures :: Stream>() {}\n'),
                               ('type T = <S as futures_core::Stream>::Item;\n', 'type T = <S as futures::Stream>::Item;\n'),
                               ('use {futures_core, std};\n', 'use {futures, std};\n'),
                               ('/// See [`futures_core::Stream`].\n', '/// See [`futures::Stream`].\n')):
            with self.subTest(text=text):
                data, records = self.mapped(text)
                self.assertEqual(data.decode(), expected)
                self.assertEqual(len(records), 1)

    def test_non_roots_substrings_and_literals_are_untouched(self):
        for text in ('my_futures_core::x();\n', 'futures_core_ext::x();\n', 'crate::futures_core::x();\n',
                     'self::futures_core::x();\n', 'value.futures_core::<T>();\n', 'let futures_core = 1;\n',
                     'let s = "futures_core::ready";\n', 'let s = r#"futures_core::x"#;\n',
                     'let s = b"\\"futures_core::x";\n', 'use std::{io, futures_core};\n',
                     "let c = '\"'; let s = \"futures_core::x\";\n", 'let s = "a\nfutures_core::b";\n'):
            with self.subTest(text=text):
                self.assertEqual(self.mapped(text), (text.encode(), []))

    def test_nested_use_paths_preserve_local_module_segments(self):
        for text in ('use crate::{futures_core::Thing};\n',
                     'use self::{futures_core::Thing as Local};\n',
                     'use super::{futures_core::{Thing, Other}};\n',
                     'use outer::{a::{futures_core::Thing}};\n',
                     'use outer::{futures_core, futures_core::Thing};\n',
                     'use crate::{\n    futures_core::Thing,\n};\n',
                     'use crate::r#futures_core::Thing;\n',
                     'use crate::{r#futures_core::Thing};\n'):
            with self.subTest(text=text):
                self.assertEqual(self.mapped(text), (text.encode(), []))

    def test_unicode_identifier_suffix_is_not_a_crate_token(self):
        for text in ('αfutures_core::Thing;\n', 'futures_coreé::Thing;\n',
                     'futures_core\u0301::Thing;\n', 'α::futures_core::Thing;\n',
                     'use crate::{αfutures_core::Thing};\n'):
            with self.subTest(text=text):
                self.assertEqual(self.mapped(text), (text.encode(), []))

    def test_root_use_groups_raw_identifiers_and_aliases_keep_their_scope(self):
        for text, expected in (
                ('use {outer::{futures_core::Thing}, futures_core::Stream};\n',
                 'use {outer::{futures_core::Thing}, futures::Stream};\n'),
                ('use {outer::{Thing as futures_core}, futures_core};\n',
                 'use {outer::{Thing as futures_core}, futures};\n'),
                ('use futures_core as futures_core;\n', 'use futures as futures_core;\n'),
                ('use r#futures_core::Stream;\n', 'use r#futures::Stream;\n'),
                ('::r#futures_core::ready!();\n', '::r#futures::ready!();\n'),
                ('use futures_core /* root */ ::Stream;\n',
                 'use futures /* root */ ::Stream;\n')):
            with self.subTest(text=text):
                data, records = self.mapped(text)
                self.assertEqual(data.decode(), expected)
                self.assertEqual(len(records), 1)

    def test_comment_use_text_cannot_change_following_code_scope(self):
        text = ('// use outer::{\n'
                'use futures_core::Stream;\n'
                '/* use crate::{ */ futures_core::ready!();\n')
        data, records = self.mapped(text)
        self.assertEqual(data.decode(), text.replace('futures_core', 'futures'))
        self.assertEqual(len(records), 2)

    def test_quotes_in_comments_and_lifetimes_do_not_hide_code(self):
        text = "// don't \"\nfn f<'a>(x: &'a u8) { futures_core::ready!(x) }\n/* \" */ futures_core::x();\n"
        data, records = self.mapped(text)
        self.assertEqual(data.count(b'futures::'), 2)
        self.assertEqual([(r['line'], r['column']) for r in records], [(2, 23), (3, 9)])

    def test_identifier_map_must_name_a_registered_consolidation(self):
        record = futures_record()
        rid = source_consolidation.receipt_id(record)
        catalog = dict(consolidations={rid: record}, consolidation_records_sha256={rid: rid}, renames={
            'src/foundation/old-name': dict(package=dict(name='old-name'), destination_name='new-name')})
        valid = [{'from': 'futures_core', 'to': 'futures', 'basis': 'consolidation:' + rid},
                 {'from': 'old_name', 'to': 'new_name', 'basis': 'rename:src/foundation/old-name'}]
        backport.validate_identifier_map(catalog, valid)
        for wrong in ([dict(valid[0], to='futures_util')], [dict(valid[0], **{'from': 'tokio'})],
                      [dict(valid[0], basis='consolidation:' + '0' * 64)], [dict(valid[0], basis='crates.io')],
                      [valid[0], valid[0]], [dict(valid[0], extra=1)], [dict(valid[1], to='other')],
                      [dict(valid[0], to='futures::x')]):
            with self.subTest(wrong=wrong), self.assertRaises(ValueError):
                backport.validate_identifier_map(catalog, wrong)
        catalog['consolidation_records_sha256'] = {}
        with self.assertRaisesRegex(ValueError, 'unregistered'):
            backport.validate_identifier_map(catalog, valid[:1])


class Layout:
    """Grouped layout fake: logical owner -> physical owner, everything else unchanged."""

    def __init__(self, root, logical, physical):
        self.root, self.logical, self.physical = Path(root), logical, physical

    def historical_path(self, path):
        return self.logical + path[len(self.physical):] if path.startswith(self.physical) else path

    def current_path(self, path):
        return self.physical + path[len(self.logical):] if path.startswith(self.logical) else path

    def read(self, path):
        return (self.root / self.current_path(path)).read_bytes()


class PathMapping(unittest.TestCase):
    def test_chain_layout_and_move_replay_and_failures(self):
        first = dict(destination_directory='src/foundation/web', destination_name='web', primary_directory='src/foundation/web-0',
                     inputs=[dict(package=dict(directory='src/foundation/web-0', origin_directory=ORIGIN))],
                     file_routes={'src/foundation/web-0': {'src/a.rs': dict(destination='src/core/a.rs'),
                                                           'src/gone.rs': dict(archived='trimmed')}},
                     input_consolidations={})
        fid = source_consolidation.receipt_id(first)
        second = dict(destination_directory='src/foundation/web', destination_name='web', primary_directory='src/foundation/web',
                      inputs=[dict(package=dict(directory='src/foundation/web', origin_directory=ORIGIN))],
                      file_routes={'src/foundation/web': {'src/core/a.rs': dict(destination='src/core/a.rs')}},
                      input_consolidations={'src/foundation/web': dict(receipt_id=fid)})
        sid = source_consolidation.receipt_id(second)
        physical = 'src/foundation/networking/web'
        catalog = dict(consolidations={fid: first, sid: second},
                       packages={physical: dict(origin_directory=ORIGIN, files_sha256={'src/moved/a.rs': '0' * 64})},
                       source_moves={'m1': dict(directory=physical, file_moves={'src/core/a.rs': 'src/moved/a.rs'})},
                       foundation_layout_receipt='L' * 64)
        origins = [dict(origin_directory=ORIGIN, original_files_sha256={'src/a.rs': '1' * 64, 'src/gone.rs': '2' * 64})]
        layout = Layout('/nonexistent', 'src/foundation/web', physical)
        derived, evidence = backport.derive_paths(catalog, origins, layout, physical, ['src/a.rs'])
        self.assertEqual(derived['src/a.rs']['local_path'], physical + '/src/moved/a.rs')
        self.assertEqual(derived['src/a.rs']['history'], ['src/foundation/web-0/src/a.rs', 'src/foundation/web/src/core/a.rs'])
        self.assertEqual(evidence, [fid, sid, 'm1', 'L' * 64])
        for path, message in (('src/gone.rs', 'no production route'), ('src/new.rs', 'origin inventory')):
            with self.subTest(path=path), self.assertRaisesRegex(ValueError, message):
                backport.derive_paths(catalog, origins, layout, physical, [path])
        catalog['consolidations'][fid]['input_consolidations'] = {}
        del catalog['consolidations'][sid]['input_consolidations']['src/foundation/web']
        with self.assertRaisesRegex(ValueError, 'exactly one terminal'):
            backport.derive_paths(catalog, origins, layout, physical, ['src/a.rs'])


class Routing(unittest.TestCase):
    REGIONS = [dict(path=TEST, line_start=BASE.count(b'\n', 0, BASE.index(b'mod tests')),
                    line_end=len(backport.split_lines(BASE)))]

    def test_production_and_test_changes_are_split(self):
        parts = backport.route_changes(BASE, FIXED, self.REGIONS, 'src/lib.rs')
        self.assertEqual(set(parts), {'production', TEST})
        base, theirs, count = parts[TEST]
        self.assertEqual((base, count), (REGION, 1))
        self.assertIn(b'fn beta_is_three', theirs)
        base, theirs, count = parts['production']
        self.assertEqual((base, count), (BASE, 1))
        self.assertIn(b'    3\n', theirs)
        self.assertNotIn(b'beta_is_three', theirs)

    def test_boundary_insertions_stay_production_and_straddles_fail(self):
        start = self.REGIONS[0]['line_start']
        lines = backport.split_lines(BASE)
        before = b''.join(lines[:start] + [b'// before tests\n'] + lines[start:])
        self.assertEqual(backport.route_changes(BASE, before, self.REGIONS, 'x')[TEST][2], 0)
        straddle = b''.join(lines[:start - 1] + [b'merged\n'] + lines[start + 1:])
        with self.assertRaisesRegex(ValueError, 'straddles'):
            backport.route_changes(BASE, straddle, self.REGIONS, 'x')


class UpstreamMaterial(unittest.TestCase):
    def test_patch_round_trip_including_missing_final_newline(self):
        for base, fixed in ((BASE, FIXED), (b'a\nb', b'a\nc'), (b'a\n', b'a\nb'), (b'a\nb', b'a\nb\n'), (b'', b'x\n')):
            with self.subTest(base=base, fixed=fixed):
                patch_bytes = backport.unified_patch(base, fixed, 'f.rs')
                self.assertEqual(backport.apply_unified(base, patch_bytes), fixed)

    def test_context_and_count_mismatch_fail(self):
        patch_bytes = backport.unified_patch(BASE, FIXED, 'src/lib.rs')
        with self.assertRaisesRegex(ValueError, 'context mismatch'):
            backport.apply_unified(BASE.replace(b'beta() -> u32', b'beta() -> u64'), patch_bytes)
        header = re.search(rb'@@ -(\d+),(\d+) \+(\d+),(\d+) @@', patch_bytes)
        forged = patch_bytes.replace(header[0], b'@@ -%s,%s +%s,%d @@' % (header[1], header[2], header[3], int(header[4]) + 1), 1)
        with self.assertRaisesRegex(ValueError, 'counts'):
            backport.apply_unified(BASE, forged)

    def test_recorded_material_drift_names_its_input(self):
        patch_bytes = backport.unified_patch(BASE, FIXED, 'src/lib.rs')
        entry = dict(upstream_path='src/lib.rs', base_sha256=backport.digest(BASE), fixed_sha256=backport.digest(FIXED),
                     patch_sha256=backport.digest(patch_bytes), base=backport.pack(BASE), patch=backport.pack(patch_bytes))
        upstream = dict(name='web', repository='r', base=dict(ref='v', sha1='a' * 40),
                        fix=dict(ref='f', sha1='b' * 40, parent='c' * 40, subject='s'), files=[entry], excluded=[])
        self.assertEqual(backport.upstream_material(upstream)['src/lib.rs'], (BASE, FIXED))
        big = b'x' * (backport.GZIP_ABOVE + 1)
        self.assertEqual(backport.pack(big)['encoding'], 'gzip')
        self.assertEqual(backport.unpack(backport.pack(big), 'big'), big)
        for key, value, message in (('base', backport.pack(BASE + b' '), 'base bytes drift'),
                                    ('patch', backport.pack(patch_bytes + b' \n'), 'patch bytes drift'),
                                    ('fixed_sha256', '0' * 64, 'fixed bytes drift')):
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, message):
                backport.upstream_material(dict(upstream, files=[dict(entry, **{key: value})]))


class Plan(Synthetic):
    def test_plan_routes_tests_maps_identifiers_and_records_the_merge(self):
        document = self.plan()
        merged = {item['local_path']: item for item in document['replay']['merged']}
        self.assertEqual(set(merged), {OWNER + '/src/lib.rs', TEST})
        self.assertEqual(document['mapping']['paths'], {'src/lib.rs': dict(production=OWNER + '/src/lib.rs', tests=[TEST])})
        self.assertEqual(document['mapping']['evidence'], [self.rid])
        self.assertEqual(document['upstream']['excluded'], ['NOTES.md'])
        self.assertEqual(document['upstream']['fix']['parent'], self.base_sha)
        self.assertEqual([(r['side'], r['line']) for r in document['replay']['identifier_replacements']],
                         [('base', 1), ('fixed', 1), ('fixed', 8)])
        review = (self.out / 'review.diff').read_bytes()
        self.assertIn(b'+    let _ = futures::task::Poll::Ready(());', review)
        self.assertIn(b'+    fn beta_is_three() {', review)
        self.assertNotIn(b'1 // local', review)  # the local edit is kept, not reverted
        self.assertEqual(self.files, {p: (self.root / p).read_bytes() for p in self.files})
        # Same inputs -> the same plan, byte for byte.
        first = (self.out / 'plan.json').read_bytes()
        self.plan()
        self.assertEqual((self.out / 'plan.json').read_bytes(), first)

    def test_conflict_exits_3_retains_attempts_and_needs_bound_resolution(self):
        changed = LOCAL.replace(b'    2\n}', b'    2 + 0\n}')
        (self.root / OWNER / 'src/lib.rs').write_bytes(changed)
        self.catalog['packages'][OWNER]['files_sha256']['src/lib.rs'] = backport.digest(changed)
        self.write_catalog()
        for _ in range(2):
            with self.assertRaises(backport.Conflict):
                self.plan()
        attempts = sorted((self.out / 'attempts').iterdir())
        self.assertEqual(len(attempts), 2)
        self.assertFalse((self.out / 'plan.json').exists())
        report = json.loads((self.out / 'conflict-report.json').read_bytes())
        conflict = report[0]['conflicts'][0]
        self.assertEqual((report[0]['file'], conflict['resolved']), (OWNER + '/src/lib.rs', False))
        self.assertIn(b'2 + 0', base64.b64decode(conflict['ours']['text']))
        self.assertIn('UNRESOLVED', (self.out / 'CONFLICTS.md').read_text())
        with redirect_stdout(io.StringIO()), patch('sys.stderr', io.StringIO()):
            self.assertEqual(backport.main(['plan', '--root', str(self.root), '--out', str(self.out),
                                            '--request', str(self.write_request())]), backport.EXIT_CONFLICT)
        resolution = dict(file=OWNER + '/src/lib.rs', hunk_index=0, conflict_sha256=conflict['sha256'],
                          resolution_base64=enc(b'    let _ = futures::task::Poll::Ready(());\n    3 + 0\n'))
        stale = dict(resolution, conflict_sha256='0' * 64)
        with self.assertRaisesRegex(ValueError, 'resolution drift'):
            self.plan(resolutions=[stale])
        document = self.plan(resolutions=[resolution])
        item = next(i for i in document['replay']['merged'] if i['region'] == 'production')
        self.assertEqual((item['hunks_conflicted'], item['resolutions']), (1, [resolution]))
        self.assertTrue(json.loads((self.out / 'conflict-report.json').read_bytes())[0]['conflicts'][0]['resolved'])
        self.assertEqual(len(list((self.out / 'attempts').iterdir())), 4)  # nothing pruned
        with self.assertRaisesRegex(ValueError, 'no current conflict'):
            self.plan(resolutions=[resolution, dict(resolution, hunk_index=1)])

    def write_request(self):
        path = self.top / 'request.json'
        path.write_text(json.dumps(self.request()))
        return path

    def test_plan_rejects_wrong_base_unlisted_file_and_repo_output(self):
        for request, message in ((dict(self.request(), upstream=dict(self.request()['upstream'], base='HEAD')), 'vcs sha'),
                                 (dict(self.request(), upstream=dict(self.request()['upstream'], files=['src/other.rs'])), 'not changed'),
                                 (dict(self.request(), identifier_map=[dict(self.identifiers[0], to='futures_util')]), 'not consolidation'),
                                 (dict(self.request(), advisory=dict(ids=['CVE-1'], disposition='hardening', basis='x')), 'advisory')):
            with self.subTest(message=message), self.assertRaisesRegex(ValueError, message):
                backport.plan(self.root, request, self.out)
        with self.assertRaisesRegex(ValueError, 'outside the repository'):
            backport.plan(self.root, self.request(), self.root / 'out')


class Apply(Synthetic):
    def apply(self, document=None, run_gates=False):
        if document is not None:
            (self.out / 'plan.json').write_text(json.dumps(document))
        with redirect_stdout(io.StringIO()):
            return backport.apply(self.root, self.out / 'plan.json', self.out, run_gates)

    def test_apply_writes_receipt_mapping_and_replays(self):
        self.plan()
        sha, row = self.apply()
        raw = (self.root / backport.RECEIPTS_DIR / (sha + '.json.gz')).read_bytes()
        self.assertEqual(backport.digest(raw), sha)
        self.assertEqual(proof_maintenance._read(self.root, sha), row)
        self.assertEqual(set(row) - proof_maintenance.FIELDS, proof_maintenance.BACKPORT_FIELDS)
        self.assertEqual(sorted(f['after'] for f in row['files']), sorted([OWNER + '/src/lib.rs', TEST]))
        test = (self.root / TEST).read_bytes()
        self.assertIn(b'fn beta_is_three', test)
        mapping = json.loads((self.root / backport.MAPPINGS / 'foundation.json').read_bytes())
        self.assertEqual(mapping['owners'][0]['files'][0]['sha256'], backport.digest(test))
        external = row['after']['external']
        self.assertEqual((external['files'][0]['sha256'], external['bytes']), (backport.digest(test), len(test)))
        self.assertEqual(row['after']['package']['files_sha256']['src/lib.rs'],
                         backport.digest((self.root / OWNER / 'src/lib.rs').read_bytes()))
        self.assertEqual(row['advisory']['scanner_identity_unchanged'], True)
        self.assertEqual(row['license'], dict(materials=['compliance/licenses/web/LICENSE'], changed=False))
        before = {path: self.files[path] for path in (OWNER + '/src/lib.rs', TEST)}
        after = {path: (self.root / path).read_bytes() for path in before}
        backport.check_replay(row, before, after, self.catalog)
        for file in row['files']:
            self.assertEqual(proof_routing.inverse_replacements(after[file["after"]], file), before[file['after']])
        self.assertEqual((self.out / 'receipt-id.txt').read_text().strip(), sha)

    def test_plan_apply_and_receipt_verification_preserve_non_root_identifiers(self):
        # New upstream hunks must keep local modules and complete Unicode tokens
        # through the actual plan/apply and receipt verification functions.
        preserved = ('use crate::{futures_core::Thing};\n'
                     'use crate::{r#futures_core::Thing as Local};\n'
                     'fn local_paths() {\n'
                     '    let _ = αfutures_core::Thing;\n'
                     '    let _ = futures_coreé::Thing;\n'
                     '    let _ = futures_core\u0301::Thing;\n'
                     '}\n').encode()
        root_use = b'use {crate::{futures_core::Thing}, futures_core::Stream};\n'
        fixed = FIXED.replace(b'    3\n}', preserved + root_use + b'    3\n}', 1)
        (self.clone / 'src/lib.rs').write_bytes(fixed)
        git(self.clone, 'commit', '-qam', 'add local paths and a real crate root')
        self.fix_sha = git(self.clone, 'rev-parse', 'HEAD')
        document = self.plan()
        sha, row = self.apply()
        production = (self.root / OWNER / 'src/lib.rs').read_bytes()
        self.assertIn(preserved, production)
        self.assertIn(b'use {crate::{futures_core::Thing}, futures::Stream};\n', production)
        self.assertIn(b'let _ = futures::task::Poll::Ready(());', production)
        before = {item['local_path']: self.files[item['local_path']]
                  for item in document['replay']['merged']}
        after = {path: (self.root / path).read_bytes() for path in before}
        stored = proof_maintenance._read(self.root, sha)
        self.assertEqual(stored, row)
        self.assertEqual(backport.check_replay(stored, before, after, self.catalog), after)
        layout = backport.FlatLayout(self.root)
        backport.check_mapping(layout, self.catalog, stored)
        backport.check_regions(layout, self.catalog, stored, historical=self.files)
        backport.check_clone(stored, self.clone)
        current = backport.after_catalog(self.catalog, OWNER,
                                         {path: (before[path], data) for path, data in after.items()})
        current[legacy.KEY] = [sha]
        backport.check_external_accounting(sha, stored, current, before, after)

    def test_replay_drift_in_each_input_is_named(self):
        self.plan()
        _, row = self.apply()
        before = {path: self.files[path] for path in (OWNER + '/src/lib.rs', TEST)}
        after = {path: (self.root / path).read_bytes() for path in before}
        upstream_file = row['upstream']['files'][0]
        tampered_base = BASE.replace(b'    1\n', b'    9\n')

        def material(value):
            value['upstream']['files'][0].update(base=backport.pack(tampered_base), base_sha256=backport.digest(tampered_base))
        cases = [
            ('local before', lambda r, b, a: b.update({TEST: REGION + b'// drift\n'}), 'local before bytes drift'),
            ('upstream base', lambda r, b, a: r['upstream']['files'][0].update(base=backport.pack(BASE + b'\n')), 'upstream base bytes drift'),
            ('upstream patch', lambda r, b, a: r['upstream']['files'][0].update(patch=backport.pack(b'')), 'upstream patch bytes drift'),
            ('rehashed upstream base', lambda r, b, a: material(r), 'upstream fixed bytes drift'),
            ('identifier map', lambda r, b, a: r['mapping'].update(identifiers=[]), 'identifier mapping drift'),
            ('resolution', lambda r, b, a: r['replay']['merged'][0]['resolutions'].append(
                dict(file=OWNER + '/src/lib.rs', hunk_index=0, conflict_sha256='0' * 64, resolution_base64='')), 'no current conflict'),
            ('after bytes', lambda r, b, a: a.update({TEST: a[TEST] + b'\n'}), 'differs from recorded after'),
            ('files', lambda r, b, a: r['files'].pop(), 'changed files differ'),
            ('license', lambda r, b, a: r['license'].update(changed=True), 'license'),
            ('advisory', lambda r, b, a: r['advisory'].update(disposition='ignored'), 'advisory'),
            ('upstream identity', lambda r, b, a: r['upstream'].update(name='hyper-fork'), 'upstream identity'),
        ]
        self.assertTrue(upstream_file['base_sha256'])
        for label, mutate, message in cases:
            r, b, a = deepcopy(row), dict(before), dict(after)
            mutate(r, b, a)
            with self.subTest(label=label), self.assertRaisesRegex(ValueError, message):
                backport.check_replay(r, b, a, self.catalog)

    def test_apply_refuses_unresolved_report_stale_plan_and_retains_failure(self):
        self.plan()
        document = json.loads((self.out / 'plan.json').read_bytes())
        (self.out / 'conflict-report.json').write_text(json.dumps([dict(file=OWNER + '/src/lib.rs', conflicts=[
            dict(index=0, sha256='0' * 64)])]))
        with self.assertRaisesRegex(ValueError, 'unresolved conflict'):
            self.apply()
        (self.out / 'conflict-report.json').unlink()
        (self.root / TEST).write_bytes(REGION + b'\n')
        with self.assertRaisesRegex(ValueError, 'changed since plan'):
            self.apply()
        (self.root / TEST).write_bytes(REGION)
        forged = deepcopy(document)
        forged['replay']['merged'][0]['result_sha256'] = '0' * 64
        with self.assertRaisesRegex(ValueError, 'does not replay'):
            self.apply(forged)
        self.assertEqual(len(list((self.out / 'attempts').glob('*-apply-failed-*'))), 3)
        self.assertEqual(self.files, {p: (self.root / p).read_bytes() for p in self.files})
        self.assertFalse((self.root / backport.RECEIPTS_DIR).exists())

    def test_failed_gate_rolls_back_every_write(self):
        self.plan(consumer_tests=dict(gates=[], commands=['false']))
        mapping = (self.root / backport.MAPPINGS / 'foundation.json').read_bytes()
        with self.assertRaisesRegex(ValueError, 'consumer gate failed'):
            self.apply(run_gates=True)
        self.assertEqual(self.files, {p: (self.root / p).read_bytes() for p in self.files})
        self.assertEqual((self.root / backport.MAPPINGS / 'foundation.json').read_bytes(), mapping)
        self.assertFalse((self.root / backport.RECEIPTS_DIR).exists())
        self.assertEqual(len(list((self.out / 'attempts').glob('*-apply-failed-*'))), 1)

    def test_apply_requires_registered_chain_tip(self):
        self.plan()
        with patch.object(legacy, 'RECEIPTS', ('0' * 64,)), self.assertRaisesRegex(ValueError, 'register and refresh'):
            self.apply()

    def assert_rejected_without_writes(self, document, message):
        before = {path: path.read_bytes() for path in self.root.rglob('*') if path.is_file()}
        prior_attempts = set((self.out / 'attempts').glob('*-apply-failed-*'))
        with self.assertRaisesRegex(ValueError, message):
            self.apply(document)
        after = {path: path.read_bytes() for path in self.root.rglob('*') if path.is_file()}
        self.assertEqual(before, after)
        self.assertFalse((self.out / 'receipt-id.txt').exists())
        attempts = set((self.out / 'attempts').glob('*-apply-failed-*')) - prior_attempts
        self.assertEqual(len(attempts), 1)
        self.assertEqual(json.loads((attempts.pop() / 'plan.json').read_bytes()), document)

    def test_recomputed_redirected_paths_cannot_authorize_a_write(self):
        document = self.plan()
        unrelated = self.root / OWNER / 'src/unrelated.rs'
        unrelated.write_bytes(LOCAL)
        outside = self.top / 'outside.rs'
        outside.write_bytes(LOCAL)
        targets = [unrelated.relative_to(self.root).as_posix(),
                   OWNER + '/src/../src/unrelated.rs', str(outside)]
        for target in targets:
            with self.subTest(target=target):
                forged = deepcopy(document)
                forged['mapping']['paths']['src/lib.rs']['production'] = target
                results = recompute_replay(forged, {target: LOCAL, TEST: REGION})
                self.assertNotEqual(results[target], LOCAL)
                self.assert_rejected_without_writes(forged, 'path mapping drift')
                self.assertEqual(outside.read_bytes(), LOCAL)

    def test_recomputed_regions_and_omitted_regions_cannot_authorize_a_write(self):
        document = self.plan()
        for omit in (False, True):
            with self.subTest(omit=omit):
                forged = deepcopy(document)
                if omit:
                    forged['mapping']['test_regions'] = {}
                    forged['mapping']['paths']['src/lib.rs']['tests'] = []
                else:
                    forged['mapping']['test_regions']['src/lib.rs'][0]['line_start'] -= 1
                recompute_replay(forged, self.files)
                self.assert_rejected_without_writes(forged, 'test region drift')

    def test_stale_mapping_evidence_and_unrouted_replay_target_are_rejected(self):
        document = self.plan()
        forged = deepcopy(document)
        forged['mapping']['evidence'].pop()
        self.assert_rejected_without_writes(forged, 'mapping evidence drift')
        forged = deepcopy(document)
        forged['replay']['merged'][0]['local_path'] = str(self.top / 'not-read.rs')
        self.assert_rejected_without_writes(forged, 'outside authenticated routes')

    def test_existing_target_and_mapping_symlinks_are_rejected(self):
        document = self.plan()
        for relative in (OWNER + '/src/lib.rs', backport.MAPPINGS + '/foundation.json'):
            with self.subTest(relative=relative):
                path = self.root / relative
                saved = self.top / 'saved-input'
                path.rename(saved)
                path.symlink_to(saved)
                try:
                    self.assert_rejected_without_writes(document, 'symlink')
                    self.assertTrue(path.is_symlink())
                finally:
                    path.unlink()
                    saved.rename(path)
        directory = self.root / OWNER / 'src'
        saved = self.top / 'saved-directory'
        directory.rename(saved)
        directory.symlink_to(saved, target_is_directory=True)
        try:
            self.assert_rejected_without_writes(document, 'symlink')
            self.assertEqual((saved / 'lib.rs').read_bytes(), LOCAL)
        finally:
            directory.unlink()
            saved.rename(directory)

    def test_missing_target_is_not_recreated_and_original_io_error_is_retained(self):
        self.plan()
        path = self.root / OWNER / 'src/lib.rs'
        path.unlink()
        with self.assertRaises(FileNotFoundError) as raised:
            self.apply()
        self.assertEqual(raised.exception.filename, str(path))
        self.assertFalse(path.exists())
        self.assertEqual((self.root / TEST).read_bytes(), REGION)
        self.assertFalse((self.root / backport.RECEIPTS_DIR).exists())

    def test_existing_receipt_is_never_overwritten_or_removed(self):
        document = self.plan()
        ours = {entry['local_path']: self.files[entry['local_path']] for entry in document['replay']['merged']}
        results = recompute_replay(document, ours)
        changes = {path: (ours[path], data) for path, data in results.items() if data != ours[path]}
        row = backport.build_receipt(document, self.catalog, changes, None)
        raw = backport.receipt_bytes(row)
        target = self.root / backport.RECEIPTS_DIR / (backport.digest(raw) + '.json.gz')
        target.parent.mkdir()
        target.write_bytes(raw)
        before = {path: path.read_bytes() for path in self.root.rglob('*') if path.is_file()}
        with self.assertRaises(FileExistsError):
            self.apply(document)
        self.assertEqual(before, {path: path.read_bytes() for path in self.root.rglob('*') if path.is_file()})

    def test_failure_to_retain_evidence_does_not_replace_original_error(self):
        document = self.plan()
        document['catalog_sha256'] = '0' * 64
        with patch.object(backport, 'retain', side_effect=PermissionError('evidence denied')):
            with self.assertRaisesRegex(ValueError, 'catalog changed') as raised:
                self.apply(document)
        self.assertIn('evidence denied', raised.exception.__notes__[0])

    def test_rollback_failure_keeps_primary_error_and_restores_other_files(self):
        self.plan()
        production = self.root / OWNER / 'src/lib.rs'
        mapping = self.root / backport.MAPPINGS / 'foundation.json'
        original_mapping = mapping.read_bytes()
        write_bytes = Path.write_bytes
        calls = 0

        def fail_production_restore(path, data):
            nonlocal calls
            if path == production:
                calls += 1
                if calls == 2:
                    raise PermissionError('source restore denied')
            return write_bytes(path, data)

        primary = OSError('consumer runner unavailable')
        with patch.object(Path, 'write_bytes', fail_production_restore), \
                patch.object(backport, 'run_commands', side_effect=primary):
            with self.assertRaises(OSError) as raised:
                self.apply(run_gates=True)
        self.assertIs(raised.exception, primary)
        self.assertIn('source restore denied', primary.__notes__[0])
        self.assertNotEqual(production.read_bytes(), LOCAL)
        self.assertEqual((self.root / TEST).read_bytes(), REGION)
        self.assertEqual(mapping.read_bytes(), original_mapping)
        self.assertFalse((self.root / backport.RECEIPTS_DIR).exists())
        error_file, = (self.out / 'attempts').glob('*-apply-failed-*/error.txt')
        self.assertIn('consumer runner unavailable', error_file.read_text())
        self.assertIn('source restore denied', error_file.read_text())


class CleanCopy(unittest.TestCase):
    def test_clean_copy_uses_only_committed_head(self):
        with tempfile.TemporaryDirectory(prefix='source-backport-clean-') as temporary:
            root = Path(temporary) / 'repo'
            tools = root / 'tools/rust-sources'
            tools.mkdir(parents=True)
            (tools / 'source_backport.py').write_text(
                'import sys, pathlib\n'
                'root = pathlib.Path(sys.argv[sys.argv.index("--root") + 1])\n'
                'assert (root / "committed.txt").is_file() and not (root / "untracked.txt").exists()\n'
                'assert pathlib.Path(__file__).resolve().is_relative_to(root.resolve())\n'
                'print("stub verify", sys.argv[sys.argv.index("--receipt") + 1])\n')
            (tools / 'manage.py').write_text('print("stub manage")\n')
            (root / 'committed.txt').write_text('x\n')
            git(root, 'init', '-q'); git(root, 'add', '.'); git(root, 'commit', '-qm', 'stub')
            (root / 'untracked.txt').write_text('working tree only\n')
            output = io.StringIO()
            with redirect_stdout(output):
                backport.verify_clean_copy(root, 'a' * 64)
            self.assertIn('stub verify ' + 'a' * 64, output.getvalue())
            self.assertIn('clean copy of HEAD', output.getvalue())
            (tools / 'manage.py').write_text('raise SystemExit(1)\n')
            git(root, 'commit', '-qam', 'failing stub')
            with redirect_stdout(io.StringIO()), self.assertRaisesRegex(ValueError, 'clean-copy command failed'):
                backport.verify_clean_copy(root, 'a' * 64)


if __name__ == '__main__':
    unittest.main()
