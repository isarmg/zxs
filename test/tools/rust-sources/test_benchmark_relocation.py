"""Regression guards for the pinned 32-file post-consolidation transition.

These checks authenticate retained source bytes; they do not run historical
benchmark programs or assert their missing providers/fixtures are available.
"""
from copy import deepcopy
import json
import hashlib
import zipfile
from pathlib import Path
import shutil
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'tools/rust-sources'))
import benchmark_relocation as relocation
import manage
import source_layout


def current_fixture_bytes(root, layout, receipt):
    """Keep ordinary implementation live; authenticate retained proof inputs."""
    result = {}
    for item in receipt['packages']:
        directory = item['directory']
        physical = source_layout.physical(root, directory)
        for path in physical.rglob('*'):
            if path.is_file():
                relative = path.relative_to(root).as_posix()
                logical = layout.historical_path(relative) if layout else relative
                result[logical] = (source_layout.read(root, logical) if path.name == 'Cargo.toml'
                                   else source_layout.safe(root, relative).read_bytes())
        external = 'test/foundation/' + directory.rsplit('/', 1)[1]
        for logical in source_layout.inventory(root, external):
            result[logical] = source_layout.read(root, logical)
    frozen = {relocation.PATH: relocation.RECEIPT, **receipt['frozen_evidence']}
    frozen.update((row['destination'], row['sha256'])
                  for item in receipt['packages'] for row in item['routes'])
    for logical, expected in frozen.items():
        data = source_layout.read(root, logical)
        if relocation.digest(data) != expected:
            raise ValueError('changed current benchmark fixture evidence: ' + logical)
        result[logical] = data
    return result


class BenchmarkRelocation(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.receipt = relocation.read(ROOT)
        current = json.loads((ROOT / 'compliance/rust-sources.json').read_text())
        # Ordinary production evolves; only retained benchmarks/evidence need
        # the strict logical-byte projection of authenticated CURRENT files.
        cls.layout = source_layout.load(ROOT)
        with source_layout.activate(cls.layout):
            cls.catalog = source_layout.project_catalog(ROOT, current)
            cls.catalog.pop(source_layout.KEY, None)
            cls.fixture_bytes = current_fixture_bytes(ROOT, cls.layout, cls.receipt)
        # Construct the exact supported after view even during a pre-refresh run.
        cls.catalog[relocation.KEY] = relocation.RECEIPT
        for item in cls.receipt['packages']:
            directory = item['directory']
            cls.catalog['packages'][directory] = dict(item['before_package'], files_sha256=item['proposed_files_sha256'])
            cls.catalog['external_tests'][directory] = item['proposed_external_tests']
            for row in cls.catalog['merges']:
                if row['directory'] == directory:
                    row['files_sha256'] = item['proposed_files_sha256']

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.cat = deepcopy(self.catalog)
        for relative, data in self.fixture_bytes.items():
            dst = self.root / relative
            dst.parent.mkdir(parents=True, exist_ok=True)
            dst.write_bytes(data)
        self.item = self.receipt['packages'][0]
        self.route = self.item['routes'][0]

    def check(self):
        return relocation.validate(self.root, self.cat, manage.files)

    def before(self):
        cat = deepcopy(self.cat)
        cat.pop(relocation.KEY, None)
        for item in self.receipt['packages']:
            directory = item['directory']
            cat['packages'][directory] = deepcopy(item['before_package'])
            cat['external_tests'][directory] = deepcopy(item['before_external_tests'])
            cat['merges'] = [p for p in cat['merges'] if p['directory'] != directory] + deepcopy(item['before_merged_packages'])
        return cat

    def reject_install(self):
        with self.assertRaises((ValueError, OSError)):
            relocation.validate(self.root, self.before(), manage.files, installing=True)

    def reject(self):
        with self.assertRaises((ValueError, OSError)):
            self.check()

    def test_exact_32_file_bytes_and_scoped_grants(self):
        receipt, grants = self.check()
        self.assertEqual(sum(len(v) for v in grants.values()), 32)
        self.assertEqual(sum(p['bytes'] for p in receipt['packages']), 145025)
        self.assertEqual(sum(p['count'] for p in receipt['packages']), 32)
        for item in receipt['packages']:
            self.assertEqual(grants[item['directory']], {(r['origin_directory'], r['historical_source'], r['sha256']) for r in item['routes']})

    def test_changed_destination_bytes(self):
        (self.root / self.route['destination']).write_bytes(b'changed')
        self.reject()

    def test_missing_destination(self):
        (self.root / self.route['destination']).unlink()
        self.reject()

    def test_extra_destination(self):
        (self.root / self.route['destination']).with_name('extra.rs').write_bytes(b'extra')
        self.reject_install()

    def test_partial_move_old_file_remains(self):
        old = self.root / self.route['source']; old.parent.mkdir(parents=True)
        shutil.copyfile(self.root / self.route['destination'], old)
        self.reject()

    def test_empty_old_directory_remains(self):
        (self.root / self.route['source']).parent.mkdir(parents=True)
        self.reject()

    def test_source_drift(self):
        (self.root / self.item['directory'] / 'src/lib.rs').write_bytes(b'changed')
        self.reject_install()

    def test_unexpected_production_disappearance(self):
        (self.root / self.item['directory'] / 'src/lib.rs').unlink()
        self.reject_install()

    def test_activation(self):
        p = self.root / self.item['directory'] / 'Cargo.toml'
        p.write_text(p.read_text().replace('autobenches = false', 'autobenches = true'))
        self.reject()

    def test_destination_symlink(self):
        p = self.root / self.route['destination']; data = p.read_bytes(); p.unlink()
        target = self.root / 'alias'; target.write_bytes(data); p.symlink_to(target)
        self.reject()

    def test_parent_symlink(self):
        p = (self.root / self.route['destination']).parent
        target = p.with_name('alias'); p.rename(target); p.symlink_to(target, target_is_directory=True)
        self.reject()

    def test_repository_alias(self):
        alias = self.root / 'alias'; alias.symlink_to(self.root, target_is_directory=True)
        with self.assertRaises(ValueError):
            relocation.validate(alias, self.cat, manage.files)

    def test_missing_receipt(self):
        (self.root / relocation.PATH).unlink(); self.reject()

    def test_missing_frozen_evidence(self):
        (self.root / next(iter(self.receipt['frozen_evidence']))).unlink(); self.reject()

    def test_rewritten_frozen_evidence(self):
        (self.root / next(iter(self.receipt['frozen_evidence']))).write_bytes(b'changed'); self.reject()

    def test_rehashed_mutated_receipt(self):
        path = self.root / relocation.PATH
        data = json.loads(path.read_text()); data['packages'][0]['routes'][0]['origin_directory'] = 'forged'
        raw = json.dumps(data).encode(); path.write_bytes(raw)
        self.cat[relocation.KEY] = relocation.digest(raw)
        self.reject()

    def test_missing_catalog_marker(self):
        del self.cat[relocation.KEY]; self.reject()
        with self.assertRaises(ValueError):
            relocation.prepare(self.root, self.cat, False, manage.files)

    def test_old_consolidation_mutation(self):
        self.cat['consolidations'][self.item['consolidation_id']]['reason'] += ' changed'
        self.reject()

    def test_missing_consolidation(self):
        del self.cat['consolidations'][self.item['consolidation_id']]; self.reject()

    def test_changed_current_package(self):
        self.cat['packages'][self.item['directory']]['name'] = 'forged'; self.reject()

    def test_changed_shared_merged_snapshot(self):
        row = next(p for p in self.cat['merges'] if p['directory'] == self.item['directory'])
        row['origin_directory'] = 'forged'; self.reject()

    def test_old_external_row_loss(self):
        self.cat['external_tests'][self.item['directory']]['files'].pop(0); self.reject()

    def test_owner_root_overlap(self):
        # Root overlap is still checked by the normal external inventory loader.
        receipt = deepcopy(self.receipt)
        receipt['packages'][0]['proposed_external_tests']['roots'].append('test/foundation')
        with patch.object(relocation, 'read', return_value=receipt):
            self.reject_install()

    def test_ordinary_source_edits_remain_refreshable(self):
        (self.root / self.item['directory'] / 'src/lib.rs').write_bytes(b'legitimate later edit')
        self.check()

    def test_current_layout_edited_source_remains_refreshable(self):
        logical = 'src/foundation/schema-validation/src/validation/error.rs'
        physical = ROOT / self.layout.current_path(logical)
        changed = physical.read_bytes() + b'\npub const LATER_MAINTENANCE: bool = true;\n'
        read_bytes = Path.read_bytes
        def read(path):
            return changed if path == physical else read_bytes(path)
        with patch.object(Path, 'read_bytes', read), source_layout.activate(self.layout):
            with self.assertRaises(ValueError):
                source_layout.read(ROOT, logical)
            current = current_fixture_bytes(ROOT, self.layout, self.receipt)
        self.assertEqual(current[logical], changed)
        (self.root / logical).write_bytes(current[logical])
        self.check()

    def test_current_benchmarks_cannot_use_historical_fallback(self):
        read_bytes = Path.read_bytes
        for item in self.receipt['packages']:
            for route in item['routes']:
                physical = ROOT / self.layout.current_path(route['destination'])
                for missing in (True, False):
                    def read(path):
                        if path == physical:
                            if missing:
                                raise FileNotFoundError(path)
                            return b'tampered current benchmark'
                        return read_bytes(path)
                    with self.subTest(path=route['destination'], missing=missing):
                        with patch.object(Path, 'read_bytes', read), source_layout.activate(self.layout):
                            with self.assertRaises((ValueError, OSError)):
                                current_fixture_bytes(ROOT, self.layout, self.receipt)

    def historical_install_view(self, fixture=None):
        # Only this install scenario needs the exact prior snapshot. Ordinary
        # verification above deliberately keeps current source and manifest edits.
        fixture = fixture or ROOT / 'test/tools/rust-sources/fixtures/benchmark-install-schema.zip'
        with zipfile.ZipFile(fixture) as archive:
            for item in self.receipt['packages']:
                directory = item['directory']
                expected = item['proposed_files_sha256']
                current = manage.files(self.root / directory)
                for relative in current.keys() - expected.keys():
                    (self.root / directory / relative).unlink()
                for relative, sha in expected.items():
                    if current.get(relative) == sha:
                        continue
                    name = directory + '/' + relative
                    edit = self.layout.edits.get(name) if self.layout else None
                    if (edit and current.get(relative) == edit['after_sha256']
                            and edit['before_sha256'] == sha):
                        data = self.layout.project_bytes(name, (self.root / name).read_bytes())
                    else:
                        self.assertLessEqual(archive.getinfo(name).file_size, 100_000)
                        data = archive.read(name)
                    self.assertEqual(hashlib.sha256(data).hexdigest(), sha)
                    path = self.root / name
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_bytes(data)
                self.assertEqual(manage.files(self.root / directory), expected)

    def test_historical_install_fixture_rejects_changed_bytes(self):
        fixture = ROOT / 'test/tools/rust-sources/fixtures/benchmark-install-schema.zip'
        changed = self.root / 'changed-fixture.zip'
        with zipfile.ZipFile(fixture) as original, zipfile.ZipFile(changed, 'w') as archive:
            for index, name in enumerate(original.namelist()):
                if index == 0:
                    (self.root / name).write_bytes(b'require historical restoration')
                archive.writestr(name, b'tampered' if index == 0 else original.read(name))
        with self.assertRaises(AssertionError):
            self.historical_install_view(changed)

    def test_unrelated_catalog_changes_allow_installation(self):
        self.historical_install_view()
        cat = self.before(); cat['unrelated_new_metadata'] = 'does not change any reviewed owner'
        relocation.validate(self.root, cat, manage.files, installing=True)

    def test_stale_install_snapshot(self):
        for field in ('packages', 'external_tests', 'merges'):
            cat = self.before()
            if field == 'packages':
                cat[field][self.item['directory']]['name'] = 'changed'
            elif field == 'external_tests':
                cat[field][self.item['directory']]['files'].pop()
            else:
                next(p for p in cat[field] if p['directory'] == self.item['directory'])['files_sha256'] = {}
            with self.subTest(field=field), self.assertRaises(ValueError):
                relocation.validate(self.root, cat, manage.files, installing=True)

    def test_unrelated_catalog_is_noop(self):
        with tempfile.TemporaryDirectory() as directory:
            self.assertEqual(relocation.validate(Path(directory), {}, manage.files), (None, {}))


    def test_same_hash_grant_does_not_authorize_other_owner(self):
        _, grants = self.check()
        from external_transfers import permits
        row = self.item['proposed_external_tests']['files'][-1]
        self.assertFalse(permits(grants, 'src/foundation/not-approved', row, row['origin_directory']))
        self.assertFalse(permits(grants, self.item['directory'], dict(row, source=row['source'] + '.other'), row['origin_directory']))

    def test_no_persistent_removal_normalization(self):
        previous = {p['directory']: deepcopy(p['before_package']) for p in self.receipt['packages']}
        self.assertEqual(relocation.remap_previous(previous, self.receipt, False), previous)
        normalized = relocation.remap_previous(previous, self.receipt, True)
        for p in self.receipt['packages']:
            self.assertEqual(normalized[p['directory']]['files_sha256'], p['proposed_files_sha256'])
        previous[self.item['directory']]['name'] = 'forged'
        with self.assertRaises(ValueError):
            relocation.remap_previous(previous, self.receipt, True)

    def test_install_loaded_mapping_must_equal_plan(self):
        current = {p['directory']: deepcopy(p['proposed_external_tests']) for p in self.receipt['packages']}
        relocation.check_current_external(current, self.receipt, True)
        rows = current[self.item['directory']]['files']
        rows[0]['path'], rows[1]['path'] = rows[1]['path'], rows[0]['path']
        rows[0]['sha256'], rows[1]['sha256'] = rows[1]['sha256'], rows[0]['sha256']
        with self.assertRaises(ValueError):
            relocation.check_current_external(current, self.receipt, True)
        relocation.check_current_external(current, self.receipt, False)

    def test_explicit_cli_only_installs_during_refresh(self):
        import io
        with patch.object(manage.sys, 'argv', ['manage.py', 'refresh', '--relocate-benchmarks']), patch.object(manage, 'refresh') as call:
            self.assertEqual(manage.main(), 0)
            self.assertTrue(call.call_args.kwargs['relocate_benchmarks'])
        with patch.object(manage.sys, 'argv', ['manage.py', 'verify', '--relocate-benchmarks']), patch.object(manage.sys, 'stderr', io.StringIO()):
            self.assertEqual(manage.main(), 1)

    def test_path_aliases_rejected(self):
        from external_tests import safe
        for path in ('../escape', '/absolute', 'test//alias', 'test/./alias', 'test/a/../b', 'test\\alias', 'test/C:alias'):
            with self.subTest(path=path), self.assertRaises(ValueError):
                safe(self.root, path)


if __name__ == '__main__':
    unittest.main()
