"""Inactive HTTP source is exact historical evidence, never advertised support."""
import importlib.util
import json
from pathlib import Path
import shutil
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('routing_gate', ROOT / '.github/scripts/check-http-routing.py')
GATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GATE)


class InactiveHttpSources(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.historical = 'src/foundation/http-routing'
        layout = GATE.source_layout.load(ROOT)
        cls.frozen_evidence = tuple(layout.receipt['frozen_evidence'])
        cls.upgrade_evidence = set()
        if layout.appended_origins:
            # The live origins file is append-only after an upgrade. Retain the
            # exact snapshot authenticated by the original layout hash as well;
            # never strip the upgrade chain or relax the fixture's proof checks.
            origins_hash = layout.receipt['frozen_evidence']['compliance/provenance/origins.json']
            cls.upgrade_evidence.add('compliance/source-layout/origins-' + origins_hash + '.json')
        cls.import_evidence = set()
        if layout.imported_owners:
            import source_imports
            catalog = json.loads((ROOT / 'compliance/rust-sources.json').read_text())
            cls.import_evidence.add('compliance/source-imports/' + catalog[source_imports.KEY] + '.json')
            for row in layout.imported_owners.values():
                cls.import_evidence.add(row['archive'])
                cls.import_evidence.update(row['original_paths'].values())
                cls.import_evidence.update(row['materials'])
        cls.correction_evidence = set()
        if layout.corrections is not None:
            from source_layout_corrections import receipt_path
            # A marked catalog authenticates the additive corrections too. Copy
            # their exact live files and targets, not a weakened catalog marker.
            cls.correction_evidence.add(receipt_path(layout.corrections.receipt_id))
            for row in layout.corrections.receipt['files']:
                cls.correction_evidence.add(row['path'])
                cls.correction_evidence.update(ref['target'] for ref in row['references'])
        # Keep the marked maintenance proof and its full live owner in the fixture.
        cls.maintenance_evidence = set()
        if layout.legacy_maintenance is not None:
            from proof_maintenance import _inventory
            adapter = layout.legacy_maintenance
            cls.maintenance_evidence.add('compliance/proof-maintenance/' + adapter.sha + '.json.gz')
            inventory, _ = _inventory(ROOT, adapter.row['after'])
            cls.maintenance_evidence.update(inventory)
        with GATE.source_layout.activate(layout):
            cls.original_sources = {
                relative: GATE.source_layout.read(ROOT, cls.historical + '/' + relative)
                for relative in (*GATE.INACTIVE_SOURCES, 'Cargo.toml', 'src/lib.rs', 'src/extract/mod.rs')
            }

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        # A small pre-layout fixture exercises the original byte contract. The
        # real repository check below independently exercises authenticated moves.
        self.component = self.root / self.historical
        for relative, original in self.original_sources.items():
            target = self.component / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(original)

    def check(self):
        GATE.verify_inactive_sources(self.root)

    def test_exact_four_original_sources_remain_inactive(self):
        self.assertEqual(len(GATE.INACTIVE_SOURCES), 4)
        self.check()
        GATE.verify_inactive_sources(ROOT)

    def test_current_catalog_retains_exact_source_identity(self):
        catalog = json.loads((ROOT / 'compliance/rust-sources.json').read_text())
        owner = catalog['packages']['src/foundation/networking/http-routing']
        with GATE.source_layout.activate(GATE.source_layout.load(ROOT)):
            for relative, expected in GATE.INACTIVE_SOURCES.items():
                current = ROOT / owner['directory'] / relative
                self.assertEqual(owner['files_sha256'][relative], GATE.hashlib.sha256(current.read_bytes()).hexdigest())
                original = GATE.source_layout.read(ROOT, 'src/foundation/http-routing/' + relative)
                self.assertEqual(GATE.hashlib.sha256(original).hexdigest(), expected)
        for relative, expected in catalog['provenance_sha256'].items():
            if '/axum-0.8.9/' in relative:
                self.assertEqual(GATE.hashlib.sha256((ROOT / relative).read_bytes()).hexdigest(), expected)

    def test_unrecorded_path_only_edit_is_rejected_by_layout_receipt(self):
        grouped = self.root / 'grouped'
        catalog = json.loads((ROOT / 'compliance/rust-sources.json').read_text())
        receipt = GATE.source_layout.receipt_path(catalog[GATE.source_layout.KEY])
        files = ['compliance/rust-sources.json', receipt, *self.frozen_evidence,
                 *sorted(self.correction_evidence), *sorted(self.upgrade_evidence),
                 *sorted(self.maintenance_evidence), *sorted(self.import_evidence)]
        owner = 'src/foundation/networking/http-routing'
        files += [owner + '/' + relative for relative in
                  (*GATE.INACTIVE_SOURCES, 'Cargo.toml', 'src/lib.rs', 'src/extract/mod.rs')]
        for relative in files:
            target = grouped / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / relative, target)
        GATE.verify_inactive_sources(grouped)
        path = grouped / owner / 'src/extract/multipart.rs'
        before = path.read_bytes()
        changed = before.replace(b'/../../../../test/', b'/../../../test/')
        self.assertNotEqual(before, changed)
        path.write_bytes(changed)
        with self.assertRaisesRegex(ValueError, 'changed or missing'):
            GATE.verify_inactive_sources(grouped)

    def test_changed_bytes_rejected_for_each_source(self):
        for relative in GATE.INACTIVE_SOURCES:
            with self.subTest(relative=relative):
                path = self.component / relative
                before = path.read_bytes()
                path.write_bytes(before + b'\n// unauthorized drift\n')
                with self.assertRaisesRegex(ValueError, 'changed or missing'):
                    self.check()
                path.write_bytes(before)

    def test_missing_source_rejected_for_each_source(self):
        for relative in GATE.INACTIVE_SOURCES:
            with self.subTest(relative=relative):
                path = self.component / relative
                before = path.read_bytes()
                path.unlink()
                with self.assertRaisesRegex(ValueError, 'changed or missing'):
                    self.check()
                path.write_bytes(before)

    def test_symlink_is_not_exact_evidence(self):
        path = self.component / 'src/extract/ws.rs'
        target = self.root / 'copied.rs'
        path.rename(target)
        path.symlink_to(target)
        with self.assertRaisesRegex(ValueError, 'symlink'):
            self.check()

    def test_each_unsupported_feature_rejected(self):
        path = self.component / 'Cargo.toml'
        before = path.read_text()
        for feature in GATE.UNSUPPORTED_FEATURES:
            with self.subTest(feature=feature):
                path.write_text(before.replace('[features]', f'[features]\n{feature} = []'))
                with self.assertRaisesRegex(ValueError, 'capability advertised'):
                    self.check()
        path.write_text(before)

    def test_each_inactive_dependency_rejected(self):
        path = self.component / 'Cargo.toml'
        before = path.read_text()
        for dependency in ('base64', 'reqwest', 'tower-http'):
            with self.subTest(dependency=dependency):
                path.write_text(before + f'\n[dependencies.{dependency}]\nversion = "1"\n')
                with self.assertRaisesRegex(ValueError, 'dependency restored'):
                    self.check()
        path.write_text(before)

    def test_restored_module_declarations_rejected(self):
        for relative, module in (('src/lib.rs', 'test_helpers'), ('src/extract/mod.rs', 'ws'), ('src/extract/mod.rs', 'multipart')):
            path = self.component / relative
            before = path.read_text()
            path.write_text(before + f'\npub mod {module};\n')
            with self.assertRaisesRegex(ValueError, 'module restored'):
                self.check()
            path.write_text(before)

    def test_duplicate_archive_rejected(self):
        path = self.root / 'compliance/provenance/retired/http-routing/source.txt'
        path.parent.mkdir(parents=True)
        path.write_text('duplicated historical material')
        with self.assertRaisesRegex(ValueError, 'duplicate'):
            self.check()


if __name__ == '__main__':
    unittest.main()
