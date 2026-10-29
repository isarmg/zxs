"""The real five demand comparisons remain reversible without rewriting origins.

Fixtures start from the first real receipt's tip: later registered real steps
(e.g. upstream backports) are inverted out of the copied files and catalog.
"""
import base64
from copy import deepcopy
import difflib
import gzip
import json
from pathlib import Path
import shutil
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'tools/rust-sources'))
import proof_legacy_maintenance as legacy
import proof_maintenance as maintenance
import proof_routing as routing
import source_layout
import source_layout_corrections


# The comment also appears in Giver; include its owner for a unique patch span.
DEMAND_SUCCESSOR_OLD = (b'impl SharedGiver {\n'
                        b'    /// Check if the `Taker` has called `want()` without parking a task.')
DEMAND_SUCCESSOR_NEW = (b'impl SharedGiver {\n'
                        b'    /// Check whether the `Taker` has called `want()` without parking a task.')


def write_receipt(root, row):
    raw = gzip.compress(json.dumps(row, sort_keys=True).encode(), mtime=0)
    sha = routing.digest(raw)
    path = root / 'compliance/proof-maintenance' / (sha + '.json.gz')
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(raw)
    return sha


def successor(root, catalog, parent, relative, old, new):
    """Synthetic source edit for proof mechanics, not an upstream/security fix."""
    before = maintenance.state(deepcopy(catalog), legacy.OWNER)
    path = legacy.OWNER + '/' + relative
    original = (root / path).read_bytes()
    if original.count(old) != 1:
        raise AssertionError('fixture replacement must identify exactly one source span')
    changed = original.replace(old, new)
    (root / path).write_bytes(changed)
    for package in [catalog['packages'][legacy.OWNER],
                    *[row for row in catalog['merges'] if row['directory'] == legacy.OWNER]]:
        package['files_sha256'][relative] = routing.digest(changed)
    return dict(schema=1, reason='Synthetic successor proof regression; not a product fix.',
                terminal=legacy.TERMINAL, parent=parent,
                layout={key: catalog.get(key) for key in
                        (source_layout.KEY, source_layout_corrections.KEY)},
                before=before, after=maintenance.state(deepcopy(catalog), legacy.OWNER),
                files=[dict(before=path, after=path, before_sha256=routing.digest(original),
                            after_sha256=routing.digest(changed),
                            replacements=[dict(old=base64.b64encode(old).decode(),
                                               new=base64.b64encode(new).decode())])])


REAL_RECEIPTS = legacy.RECEIPTS


def first_tip(catalog):
    """Catalog and file bytes at the first real receipt's tip, inverting later real steps."""
    rows = [maintenance._read(ROOT, sha) for sha in REAL_RECEIPTS]
    first = rows[0]['after']
    catalog['packages'][legacy.OWNER] = deepcopy(first['package'])
    catalog['merges'] = [row for row in catalog['merges'] if row['directory'] != legacy.OWNER] + deepcopy(first['merges'])
    catalog['external_tests'][legacy.OWNER] = deepcopy(first['external'])
    files = {}
    for row in reversed(rows[1:]):
        for file in row['files']:
            data = files.get(file['after'], (ROOT / file['after']).read_bytes())
            files[file['before']] = routing.inverse_replacements(data, file)
    return catalog, files


class HttpChainFixture(unittest.TestCase):
    """Disposable owner copy whose chain starts with a re-created first demand receipt."""

    @classmethod
    def setUpClass(cls):
        cls.catalog_template, cls.rewound = first_tip(json.loads((ROOT / 'compliance/rust-sources.json').read_bytes()))
        layout_id = cls.catalog_template[source_layout.KEY]
        cls.layout_receipt = source_layout.read_receipt(ROOT, layout_id)
        cls.layout_row = next(row for row in cls.layout_receipt['packages']
                              if row['directory'] == legacy.LOGICAL)
        original = (ROOT / legacy.OWNER / legacy.FILE).read_bytes()
        for state in ('Want', 'Closed'):
            for operator in ('==', '!='):
                original = original.replace(
                    f'load(SeqCst) {operator} usize::from(State::{state})'.encode(),
                    f'load(SeqCst) {operator} State::{state}.into()'.encode())
        cls.original = original
        cls.changed = original
        for state in ('Want', 'Closed'):
            for operator in ('==', '!='):
                cls.changed = cls.changed.replace(
                    f'load(SeqCst) {operator} State::{state}.into()'.encode(),
                    f'load(SeqCst) {operator} usize::from(State::{state})'.encode())
        assert sum(a != b for a, b in zip(original.splitlines(), cls.changed.splitlines())) == 5
        assert routing.digest(original) == cls.layout_row['files_sha256'][legacy.FILE]

    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='legacy-http-maintenance-')
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.previous = deepcopy(self.catalog_template)
        self.previous.pop(legacy.KEY, None)
        for package in [self.previous['packages'][legacy.OWNER],
                        *[row for row in self.previous['merges'] if row['directory'] == legacy.OWNER]]:
            package['files_sha256'][legacy.FILE] = routing.digest(self.original)
        before = maintenance.state(deepcopy(self.previous), legacy.OWNER)
        after = deepcopy(before)
        after['package']['files_sha256'][legacy.FILE] = routing.digest(self.changed)
        for package in after['merges']:
            package['files_sha256'][legacy.FILE] = routing.digest(self.changed)
        roots = [legacy.OWNER, *before['external']['roots']]
        for relative in roots:
            shutil.copytree(ROOT / relative, self.root / relative)
        for path, data in self.rewound.items():
            (self.root / path).write_bytes(data)
        self.path = self.root / legacy.OWNER / legacy.FILE
        self.path.write_bytes(self.changed)
        self.current = deepcopy(self.previous)
        self.current['packages'][legacy.OWNER] = deepcopy(after['package'])
        self.current['merges'] = [row for row in self.current['merges']
                                  if row['directory'] != legacy.OWNER] + deepcopy(after['merges'])
        lines_before, lines_after = self.original.splitlines(keepends=True), self.changed.splitlines(keepends=True)
        replacements = []
        for group in difflib.SequenceMatcher(None, lines_before, lines_after).get_grouped_opcodes(3):
            old = b''.join(lines_before[group[0][1]:group[-1][2]])
            new = b''.join(lines_after[group[0][3]:group[-1][4]])
            replacements.append({key: base64.b64encode(value).decode()
                                 for key, value in (('old', old), ('new', new))})
        physical = legacy.OWNER + '/' + legacy.FILE
        self.row = dict(schema=1, reason='Disambiguate the five real atomic demand comparisons.',
                        terminal=legacy.TERMINAL, parent=None,
                        layout={key: self.previous.get(key) for key in
                                (source_layout.KEY, source_layout_corrections.KEY)},
                        before=before, after=after,
                        files=[dict(before=physical, after=physical,
                                    before_sha256=routing.digest(self.original),
                                    after_sha256=routing.digest(self.changed), replacements=replacements)])
        self.layout = SimpleNamespace(
            root=self.root, receipt_id=self.previous[source_layout.KEY],
            receipt=deepcopy(self.layout_receipt), rows={legacy.LOGICAL: deepcopy(self.layout_row)},
            moves={legacy.LOGICAL: legacy.OWNER}, reverse={legacy.OWNER: legacy.LOGICAL},
            edits=deepcopy(self.layout_receipt['file_edits']), corrections=None,
            descendant_files={}, upgraded_owners={})
        corrections_id = self.previous.get(source_layout_corrections.KEY)
        if corrections_id is not None:
            correction_receipt = source_layout_corrections.read_receipt(ROOT, corrections_id)
            corrections = object.__new__(source_layout_corrections.Corrections)
            corrections.rows = {row['path']: row for row in correction_receipt['files']}
            self.layout.corrections = corrections
        self.layout.current_path = lambda value: value.replace(legacy.LOGICAL + '/', legacy.OWNER + '/') if value != legacy.LOGICAL else legacy.OWNER
        self.layout.historical_path = lambda value: value.replace(legacy.OWNER + '/', legacy.LOGICAL + '/') if value != legacy.OWNER else legacy.LOGICAL
        self.layout.project_layout_bytes = lambda path, data: source_layout.Layout.project_layout_bytes(self.layout, path, data)
        self.pin = patch.object(legacy, 'RECEIPTS', ())
        self.pin.start()
        self.addCleanup(self.pin.stop)
        self.rows = [self.row]
        self.register()

    def register(self):
        ids = tuple(write_receipt(self.root, row) for row in self.rows)
        self.sha = ids[-1]
        self.receipt_path = self.root / 'compliance/proof-maintenance' / (self.sha + '.json.gz')
        legacy.RECEIPTS = ids
        self.current[legacy.KEY] = list(ids)

    def append_successor(self, relative=legacy.FILE,
                         old=DEMAND_SUCCESSOR_OLD, new=DEMAND_SUCCESSOR_NEW):
        row = successor(self.root, self.current, self.sha, relative, old, new)
        self.rows.append(row)
        self.register()
        return row

    def adapter(self):
        return legacy.load(self.layout, self.current)


class LegacyHttpMaintenance(HttpChainFixture):
    def test_five_real_comparisons_replay_before_immutable_layout_inverse(self):
        adapter = self.adapter()
        logical = legacy.LOGICAL + '/' + legacy.FILE
        restored = adapter.read(logical, self.changed)
        self.assertEqual(restored, self.original)
        self.assertEqual(adapter.project_hash(logical, routing.digest(self.changed)), routing.digest(self.original))
        edit = self.layout.edits[logical]
        historical = source_layout.inverse(logical, restored, edit, self.layout.moves)
        self.assertEqual(routing.digest(historical), edit['before_sha256'])
        self.assertEqual(self.path.read_bytes(), self.changed)
        adapter.validate_catalog(self.current)
        adapter.validate_refresh(self.previous, self.current)

    def test_unrelated_projection_is_unchanged(self):
        adapter = self.adapter()
        self.assertEqual(adapter.read('src/foundation/unrelated/lib.rs', b'unrelated'), b'unrelated')
        self.assertEqual(adapter.project_hash('src/foundation/unrelated/lib.rs', '0' * 64), '0' * 64)

    def test_tampered_live_demand_is_rejected(self):
        self.path.write_bytes(self.changed + b'\n')
        with self.assertRaises(ValueError):
            self.adapter()

    def test_missing_live_demand_is_rejected(self):
        self.path.unlink()
        with self.assertRaises((ValueError, FileNotFoundError)):
            self.adapter()

    def test_unlisted_live_file_is_rejected(self):
        (self.root / legacy.OWNER / 'unlisted.rs').write_text('const UNLISTED: bool = true;\n')
        with self.assertRaises(ValueError):
            self.adapter()

    def test_unchanged_source_file_still_requires_exact_bytes(self):
        path = self.root / legacy.OWNER / 'src/lib.rs'
        path.write_bytes(path.read_bytes() + b'\n')
        with self.assertRaises(ValueError):
            self.adapter()

    def test_unchanged_external_test_still_requires_exact_bytes(self):
        path = self.root / self.row['after']['external']['files'][0]['path']
        path.write_bytes(path.read_bytes() + b'\n')
        with self.assertRaises(ValueError):
            self.adapter()

    def test_wrong_layout_binding_is_rejected_even_with_repinned_receipt(self):
        self.row['layout'][source_layout.KEY] = '0' * 64
        self.register()
        with self.assertRaises(ValueError):
            self.adapter()

    def test_wrong_terminal_is_rejected_even_with_repinned_receipt(self):
        self.row['terminal'] = '0' * 64
        self.register()
        with self.assertRaises(ValueError):
            self.adapter()

    def test_wrong_owner_is_rejected_even_with_repinned_receipt(self):
        self.row['before']['package']['directory'] = 'src/foundation/networking/unrelated'
        self.register()
        with self.assertRaises(ValueError):
            self.adapter()

    def test_omitted_changed_file_is_rejected(self):
        self.row['files'] = []
        self.register()
        with self.assertRaises(ValueError):
            self.adapter()

    def test_unauthorized_source_file_is_rejected(self):
        self.row['files'][0]['before'] = legacy.OWNER + '/src/lib.rs'
        self.row['files'][0]['after'] = legacy.OWNER + '/src/lib.rs'
        self.register()
        with self.assertRaises(ValueError):
            self.adapter()

    def test_receipt_bytes_cannot_change_under_existing_pin(self):
        self.receipt_path.write_bytes(self.receipt_path.read_bytes() + b'\n')
        with self.assertRaises(ValueError):
            self.adapter()

    def test_old_consolidation_history_cannot_be_changed(self):
        self.current['consolidations'][legacy.TERMINAL]['reason'] += ' rewritten'
        with self.assertRaises(ValueError):
            self.adapter()

    def test_layout_predecessor_hash_cannot_be_changed(self):
        self.layout.rows[legacy.LOGICAL]['files_sha256'][legacy.FILE] = '0' * 64
        with self.assertRaises(ValueError):
            self.adapter()

    def test_catalog_cannot_claim_stale_demand_hash(self):
        self.current['packages'][legacy.OWNER]['files_sha256'][legacy.FILE] = routing.digest(self.original)
        with self.assertRaises(ValueError):
            self.adapter()

    def test_missing_registration_is_not_a_verify_pass(self):
        self.current.pop(legacy.KEY)
        with self.assertRaises(ValueError):
            self.adapter()

    def test_prospective_refresh_accepts_only_exact_previous_owner(self):
        adapter = legacy.load(self.layout, self.previous, prospective=True)
        adapter.validate_refresh(self.previous, self.current)
        self.assertEqual(adapter.read(legacy.LOGICAL + '/' + legacy.FILE, self.changed), self.original)

    def test_prospective_refresh_rejects_different_previous_owner(self):
        self.previous['packages'][legacy.OWNER]['files_sha256'][legacy.FILE] = '0' * 64
        with self.assertRaises(ValueError):
            legacy.load(self.layout, self.previous, prospective=True)

    def test_refresh_rejects_stale_previous_owner(self):
        adapter = self.adapter()
        self.previous['packages'][legacy.OWNER]['files_sha256'][legacy.FILE] = '0' * 64
        with self.assertRaises(ValueError):
            adapter.validate_refresh(self.previous, self.current)

    def test_two_successors_restore_real_first_step_and_preserve_old_receipts(self):
        first = self.receipt_path
        first_bytes = first.read_bytes()
        original_lib = (self.root / legacy.OWNER / 'src/lib.rs').read_bytes()
        previous = deepcopy(self.current)
        self.append_successor()
        second = self.receipt_path
        second_bytes = second.read_bytes()
        self.adapter().validate_refresh(previous, self.current)
        previous = deepcopy(self.current)
        self.append_successor('src/lib.rs', b'//! # HTTP transport', b'//! # HTTP transport fixture')
        adapter = self.adapter()
        adapter.validate_refresh(previous, self.current)
        adapter.validate_refresh(self.current, self.current)
        self.assertEqual(adapter.receipt_ids, legacy.RECEIPTS)
        self.assertEqual(adapter.sha, legacy.RECEIPTS[-1])
        self.assertEqual(adapter.row, self.rows[-1])
        logical = legacy.LOGICAL + '/' + legacy.FILE
        restored = adapter.read(logical, self.path.read_bytes())
        self.assertEqual(restored, self.original)
        self.assertEqual(adapter.project_hash(logical, routing.digest(self.path.read_bytes())),
                         routing.digest(self.original))
        library = (self.root / legacy.OWNER / 'src/lib.rs').read_bytes()
        self.assertEqual(adapter.read(legacy.LOGICAL + '/src/lib.rs', library), original_lib)
        self.assertEqual(adapter.project_hash(legacy.LOGICAL + '/src/lib.rs', routing.digest(library)),
                         routing.digest(original_lib))
        edit = self.layout.edits[logical]
        historical = source_layout.inverse(logical, restored, edit, self.layout.moves)
        self.assertEqual(routing.digest(historical), edit['before_sha256'])
        self.assertEqual(first.read_bytes(), first_bytes)
        self.assertEqual(second.read_bytes(), second_bytes)

    def test_successor_live_bytes_are_rechecked_after_authentication(self):
        self.append_successor()
        adapter = self.adapter()
        self.path.write_bytes(self.path.read_bytes() + b'// drift\n')
        with self.assertRaisesRegex(ValueError, 'live owner'):
            adapter.read(legacy.LOGICAL + '/' + legacy.FILE, self.path.read_bytes())

    def test_successor_cannot_skip_registration_or_multiple_refresh_steps(self):
        before_second = deepcopy(self.current)
        self.append_successor()
        with self.assertRaises(ValueError):
            legacy.load(self.layout, before_second)
        adapter = legacy.load(self.layout, before_second, prospective=True)
        adapter.validate_refresh(before_second, self.current)
        self.append_successor('src/lib.rs', b'//! # HTTP transport', b'//! # HTTP transport fixture')
        with self.assertRaisesRegex(ValueError, 'immediate predecessor'):
            legacy.load(self.layout, before_second, prospective=True)

    def test_successor_reordered_omitted_or_duplicate_receipts_fail(self):
        self.append_successor()
        ids = legacy.RECEIPTS
        for wrong in (ids[::-1], ids[1:], ids + ids[:1]):
            with self.subTest(ids=wrong), patch.object(legacy, 'RECEIPTS', wrong):
                self.current[legacy.KEY] = list(wrong)
                with self.assertRaises(ValueError):
                    self.adapter()

    def test_successor_requires_exact_parent_and_full_before_snapshot(self):
        row = self.append_successor()
        original = deepcopy(row)
        def forge_ancestry(value):
            for state in ('before', 'after'):
                value[state]['package']['description'] = 'forged'
        for mutate in (lambda value: value.update(parent='0' * 64),
                       forge_ancestry):
            with self.subTest(mutate=mutate):
                row.clear(); row.update(deepcopy(original)); mutate(row)
                self.register()
                with self.assertRaises(ValueError):
                    self.adapter()

    def test_successor_requires_every_merged_source_snapshot(self):
        row = self.append_successor()
        row['after']['merges'][0]['files_sha256'][legacy.FILE] = '0' * 64
        self.register()
        with self.assertRaisesRegex(ValueError, 'owner identity'):
            self.adapter()

    def test_successor_cannot_change_identity_license_or_upstream(self):
        row = self.append_successor()
        original = deepcopy(row)
        for field in ('name', 'version', 'origin_directory', 'license', 'license_materials', 'upstream'):
            with self.subTest(field=field):
                row.clear(); row.update(deepcopy(original))
                row['after']['package'][field] = 'forged'
                self.register()
                with self.assertRaises(ValueError):
                    self.adapter()

    def test_successor_cannot_omit_duplicate_or_forge_changed_files(self):
        row = self.append_successor()
        original = deepcopy(row)
        for files in ([], [original['files'][0]] * 2,
                      [dict(original['files'][0], after_sha256='0' * 64)]):
            with self.subTest(files=files):
                row['files'] = deepcopy(files)
                self.register()
                with self.assertRaises(ValueError):
                    self.adapter()

    def test_one_successor_must_cover_both_changed_sources(self):
        first = successor(self.root, self.current, self.sha, legacy.FILE,
                          DEMAND_SUCCESSOR_OLD, DEMAND_SUCCESSOR_NEW)
        second = successor(self.root, self.current, self.sha, 'src/lib.rs',
                           b'//! # HTTP transport', b'//! # HTTP transport fixture')
        first['after'] = second['after']
        first['files'].extend(second['files'])
        self.rows.append(first)
        self.register()
        self.adapter()
        first['files'].pop()
        self.register()
        with self.assertRaisesRegex(ValueError, 'coverage'):
            self.adapter()

    def test_third_step_requires_middle_receipt_file_and_registered_identity(self):
        self.append_successor()
        middle = self.receipt_path
        self.append_successor('src/lib.rs', b'//! # HTTP transport', b'//! # HTTP transport fixture')
        ids = legacy.RECEIPTS
        with patch.object(legacy, 'RECEIPTS', (ids[0], ids[2])):
            self.current[legacy.KEY] = [ids[0], ids[2]]
            with self.assertRaisesRegex(ValueError, 'ancestry'):
                self.adapter()
        self.current[legacy.KEY] = list(ids)
        middle.unlink()
        with self.assertRaises(FileNotFoundError):
            self.adapter()

    def test_successor_replacement_budget_and_inverse_are_enforced(self):
        row = self.append_successor()
        with patch.object(routing, 'MAX_PATCH_BYTES', 1), self.assertRaises(ValueError):
            self.adapter()
        row['files'][0]['replacements'][0]['old'] = base64.b64encode(b'not the historical span').decode()
        self.register()
        with self.assertRaisesRegex(ValueError, 'historical content'):
            self.adapter()

    def test_successor_cannot_change_manifest_or_add_source(self):
        self.append_successor('Cargo.toml', b'name = "http-transport"', b'name = "forged"')
        with self.assertRaisesRegex(ValueError, 'Rust source content'):
            self.adapter()

    def test_successor_cannot_change_source_file_set(self):
        row = self.append_successor()
        for package in [row['after']['package'], *row['after']['merges']]:
            package['files_sha256']['src/new.rs'] = routing.digest(b'// new\n')
        self.register()
        with self.assertRaisesRegex(ValueError, 'inventory changed|file sets'):
            self.adapter()

    def test_successor_cannot_change_corrected_external_test_even_with_new_hashes(self):
        row = self.append_successor()
        path = 'test/foundation/http-transport/adapters/unit/src/server/conn/auto/mod/guarded_tests_1135.rs'
        self.assertIn(path, self.layout.corrections.rows)
        external = row['after']['external']
        entry = next(entry for entry in external['files'] if entry['path'] == path)
        data = (self.root / path).read_bytes() + b'// synthetic external drift\n'
        (self.root / path).write_bytes(data)
        external['bytes'] += len(b'// synthetic external drift\n')
        entry['sha256'] = routing.digest(data)
        self.current['external_tests'][legacy.OWNER] = deepcopy(external)
        self.register()
        with self.assertRaisesRegex(ValueError, 'external tests'):
            self.adapter()

    def test_successor_missing_live_file_and_changed_first_receipt_fail(self):
        first = self.receipt_path
        first_bytes = first.read_bytes()
        self.append_successor()
        first.write_bytes(first_bytes + b'\n')
        with self.assertRaisesRegex(ValueError, 'immutable maintenance'):
            self.adapter()
        first.write_bytes(first_bytes)
        self.path.unlink()
        with self.assertRaises((ValueError, FileNotFoundError)):
            self.adapter()


if __name__ == '__main__':
    unittest.main()
