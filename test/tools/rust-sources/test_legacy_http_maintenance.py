"""The real five demand comparisons remain reversible without rewriting origins."""
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


class LegacyHttpMaintenance(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.catalog_template = json.loads((ROOT / 'compliance/rust-sources.json').read_bytes())
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
            edits=deepcopy(self.layout_receipt['file_edits']), corrections=None)
        corrections_id = self.previous.get(source_layout_corrections.KEY)
        if corrections_id is not None:
            correction_receipt = source_layout_corrections.read_receipt(ROOT, corrections_id)
            corrections = object.__new__(source_layout_corrections.Corrections)
            corrections.rows = {row['path']: row for row in correction_receipt['files']}
            self.layout.corrections = corrections
        self.layout.current_path = lambda value: value.replace(legacy.LOGICAL + '/', legacy.OWNER + '/') if value != legacy.LOGICAL else legacy.OWNER
        self.layout.historical_path = lambda value: value.replace(legacy.OWNER + '/', legacy.LOGICAL + '/') if value != legacy.OWNER else legacy.LOGICAL
        self.pin = patch.object(legacy, 'RECEIPTS', ())
        self.pin.start()
        self.addCleanup(self.pin.stop)
        self.register()

    def register(self):
        raw = gzip.compress(json.dumps(self.row, sort_keys=True).encode(), mtime=0)
        self.sha = routing.digest(raw)
        self.receipt_path = self.root / 'compliance/proof-maintenance' / (self.sha + '.json.gz')
        self.receipt_path.parent.mkdir(parents=True, exist_ok=True)
        self.receipt_path.write_bytes(raw)
        legacy.RECEIPTS = (self.sha,)
        self.current[legacy.KEY] = [self.sha]

    def adapter(self):
        return legacy.load(self.layout, self.current)

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


if __name__ == '__main__':
    unittest.main()
