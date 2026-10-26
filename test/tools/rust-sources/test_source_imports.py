"""Exact text-source import boundary tests; synthetic archives are never executed."""
from copy import deepcopy
import io
import json
from pathlib import Path
import sys
import tarfile
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / 'tools/rust-sources'))
import source_imports as imports
from source_layout import canonical, digest


class ImportTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.sources = {'wasm-text-parser': ('web-runtime', 'wast', '245.0.1', ''),
                        'wasm-text-printer': ('web-runtime', 'wasmprinter', '0.245.1', ''),
                        'unicode-display-width': ('text-unicode', 'unicode-width', '0.2.2', '')}
        self.rows = []
        for local, (group, name, version, _) in self.sources.items():
            identity = name + '-' + version
            original = {'Cargo.toml': b'[package]\n', 'src/lib.rs': b'// original\n', 'LICENSE': b'legal text\n'}
            stream = io.BytesIO()
            with tarfile.open(fileobj=stream, mode='w:gz') as archive:
                for path, content in original.items():
                    member = tarfile.TarInfo(identity + '/' + path)
                    member.size = len(content)
                    archive.addfile(member, io.BytesIO(content))
            archive = stream.getvalue(); checksum = digest(archive)
            self.sources[local] = (group, name, version, checksum)
            base = 'compliance/provenance/packages/' + identity + '/'
            archive_path = base + 'release.crate'; self.write(archive_path, archive)
            paths = {path: base + 'original/' + path + '.txt' for path in original}
            for path, content in original.items():
                self.write(paths[path], content)
            upstream = dict(name=name, version=version, archive_sha256=checksum,
                            archive_url=f'https://static.crates.io/crates/{name}/{identity}.crate')
            origin = dict(origin_directory='vendor/rust/' + identity, upstream=upstream,
                license='MIT', repository='https://example.test/source', vcs={'git': {'sha1': 'a' * 40}},
                original_files_sha256={p: digest(v) for p, v in original.items()},
                central_license_materials=[paths['LICENSE']], central_original_metadata=[paths['Cargo.toml']],
                central_original_documentation=[])
            package = dict(name=local, version=version, directory='src/foundation/' + group + '/' + local,
                origin_directory=origin['origin_directory'], upstream=upstream, license=origin['license'],
                repository=origin['repository'], vcs=origin['vcs'], license_materials=origin['central_license_materials'],
                files_sha256={'src/lib.rs': digest(b'// current\n')})
            self.rows.append(dict(package=package, origin=origin, archive=archive_path, original_paths=paths,
                materials={}, project_authored_files={}, omissions='No framework or compiler-internal capabilities.',
                external_tests={'files': []},
                source_disposition={p: {'current_paths': [package['directory'] + '/src/lib.rs'] if p == 'src/lib.rs' else [],
                                        'reason': 'Retained original and reviewed adaptation.'} for p in original}))
        self.plan = dict(schema=1, reason='Three actual binding-test consumers.',
            predecessor_catalog_sha256='c' * 64, predecessor_layout='d' * 64, packages=self.rows)
        self.pin()

    def patch(self, owner, name, value):
        patcher = patch.object(owner, name, value)
        patcher.start(); self.addCleanup(patcher.stop)

    def write(self, path, content):
        target = self.root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(content)

    def pin(self):
        raw = canonical(self.plan); self.marker = digest(raw)
        self.write('compliance/source-imports/' + self.marker + '.json', raw)
        self.patch(imports, 'RECEIPTS', (self.marker,))

    def catalog(self):
        return {imports.KEY: self.marker, 'packages': {r['package']['directory']: deepcopy(r['package']) for r in self.rows}}

    def layout(self):
        return SimpleNamespace(root=self.root, receipt_id='d' * 64, reverse={},
            appended_origins={r['origin']['origin_directory']: r['origin'] for r in self.rows})

    def validate(self):
        return imports.current(self.layout(), self.catalog())

    def test_exact_three_sources(self):
        owners, origins = self.validate()
        self.assertEqual(len(owners), 3)
        self.assertEqual(len(origins), 3)

    def test_unreviewed_receipt(self):
        with self.assertRaisesRegex(ValueError, 'unreviewed'):
            imports.read(self.root, 'e' * 64)

    def test_tampered_plan(self):
        self.write('compliance/source-imports/' + self.marker + '.json', b'{}')
        with self.assertRaisesRegex(ValueError, 'receipt changed'):
            self.validate()

    def test_duplicate_origin_or_responsibility(self):
        self.plan['packages'][1] = deepcopy(self.rows[0]); self.pin()
        with self.assertRaisesRegex(ValueError, 'duplicate imported'):
            self.validate()

    def test_archive_tamper(self):
        self.write(self.rows[0]['archive'], b'not the release')
        with self.assertRaisesRegex(ValueError, 'archive checksum'):
            self.validate()

    def test_license_tamper(self):
        self.write(self.rows[0]['origin']['central_license_materials'][0], b'changed')
        with self.assertRaisesRegex(ValueError, 'material changed'):
            self.validate()

    def test_missing_original(self):
        del self.rows[0]['original_paths']['src/lib.rs']; self.pin()
        with self.assertRaisesRegex(ValueError, 'coverage mismatch'):
            self.validate()

    def test_path_escape(self):
        self.rows[0]['original_paths']['src/lib.rs'] = '../outside'; self.pin()
        with self.assertRaisesRegex(ValueError, 'destination mismatch'):
            self.validate()

    def test_symlink_material(self):
        path = self.root / self.rows[0]['original_paths']['src/lib.rs']
        path.unlink(); path.symlink_to('/tmp')
        with self.assertRaisesRegex(ValueError, 'symlink'):
            self.validate()

    def test_retired_resurrection(self):
        catalog = self.catalog(); catalog['retired'] = {self.rows[0]['package']['directory']: {}}
        with self.assertRaisesRegex(ValueError, 'identity or origin drift'):
            imports.current(self.layout(), catalog)

    def test_old_directory_collision(self):
        layout = self.layout(); layout.reverse[self.rows[0]['package']['directory']] = 'old'
        with self.assertRaisesRegex(ValueError, 'identity or origin drift'):
            imports.current(layout, self.catalog())

    def test_changed_origin(self):
        layout = self.layout(); layout.appended_origins = deepcopy(layout.appended_origins)
        layout.appended_origins[self.rows[0]['origin']['origin_directory']]['license'] = 'changed'
        with self.assertRaisesRegex(ValueError, 'identity or origin drift'):
            imports.current(layout, self.catalog())

    def test_wrong_predecessor(self):
        layout = self.layout(); layout.receipt_id = 'e' * 64
        with self.assertRaisesRegex(ValueError, 'predecessor layout'):
            imports.current(layout, self.catalog())

    def test_unregistered_physical_directory(self):
        import manage
        from unittest.mock import patch
        layout = self.layout()
        layout.receipt = {'categories': {'web-runtime': 'tools'}}
        layout.imported_owners = {r['package']['directory']: r for r in self.rows}
        self.write('src/foundation/web-runtime/unregistered/Cargo.toml', b'[package]\n')
        with patch.object(manage.source_layout, 'load', return_value=layout):
            with self.assertRaisesRegex(ValueError, 'grouped source inventory drift'):
                manage.source_directories(self.root, {'integrated_sources': []})

    def test_archive_link_rejected_even_with_matching_reviewed_hash(self):
        row = self.rows[0]; identity = row['origin']['upstream']['name'] + '-' + row['package']['version']
        stream = io.BytesIO()
        with tarfile.open(fileobj=stream, mode='w:gz') as archive:
            member = tarfile.TarInfo(identity + '/link')
            member.type = tarfile.SYMTYPE; member.linkname = '/tmp/outside'
            archive.addfile(member)
        raw = stream.getvalue(); self.write(row['archive'], raw)
        row['origin']['upstream']['archive_sha256'] = digest(raw)
        self.pin()
        with self.assertRaisesRegex(ValueError, 'non-file'):
            self.validate()

    def test_special_material_rejected_without_reading(self):
        import os
        path = self.root / self.rows[0]['original_paths']['src/lib.rs']
        path.unlink(); os.mkfifo(path)
        with self.assertRaisesRegex(ValueError, 'special'):
            self.validate()

    def test_refresh_restores_origins_only_before_catalog_commit(self):
        import manage
        original_catalog = b'{"packages": {}}'
        original_origins = b'{"packages": []}'
        self.write(manage.CATALOG, original_catalog)
        self.write(manage.ORIGINS, original_origins)

        def prepared(root, previous, marker, raw):
            self.assertEqual(raw, original_catalog)
            self.write(manage.ORIGINS, b'{"packages": ["new"]}')
            return previous

        for committed in (False, True):
            with self.subTest(committed=committed):
                self.write(manage.CATALOG, original_catalog)
                self.write(manage.ORIGINS, original_origins)

                def failed(*args):
                    if committed:
                        manage.atomic_write(self.root / manage.CATALOG, b'{"packages": {"new": {}}}')
                    raise ValueError('injected documentation write failure' if committed else 'injected downstream failure')

                with patch.object(imports, 'prepare', side_effect=prepared), \
                     patch.object(manage.source_layout, 'load', return_value=None), \
                     patch.object(manage, '_refresh', side_effect=failed):
                    with self.assertRaisesRegex(ValueError, 'documentation write failure' if committed else 'downstream failure'):
                        manage.refresh(self.root, import_plan_id=self.marker)
                expected = b'{"packages": ["new"]}' if committed else original_origins
                self.assertEqual((self.root / manage.ORIGINS).read_bytes(), expected)

    def test_catalog_replace_failure_preserves_complete_prior_bytes(self):
        import manage
        path = self.root / 'catalog.json'
        path.write_bytes(b'complete predecessor')
        with patch.object(manage.os, 'replace', side_effect=OSError('injected ENOSPC before rename')):
            with self.assertRaisesRegex(OSError, 'ENOSPC'):
                manage.atomic_write(path, b'complete successor')
        self.assertEqual(path.read_bytes(), b'complete predecessor')
        self.assertEqual(list(self.root.glob('.catalog.json.*')), [])

    def test_catalog_write_failure_after_append_restores_origins(self):
        import manage
        original_catalog = b'{"packages": {}}'
        original_origins = b'{"packages": []}'
        self.write(manage.CATALOG, original_catalog)
        self.write(manage.ORIGINS, original_origins)

        def prepared(root, previous, marker, raw):
            manage.atomic_write(root / manage.ORIGINS, b'{"packages": ["new"]}')
            return previous

        def failed(*args):
            manage.atomic_write(self.root / manage.CATALOG, b'{"packages": {"new": {}}}')

        replace = manage.os.replace
        def no_catalog(source, destination):
            if destination == self.root / manage.CATALOG:
                raise OSError('injected catalog write failure')
            return replace(source, destination)

        with patch.object(imports, 'prepare', side_effect=prepared), \
             patch.object(manage.source_layout, 'load', return_value=None), \
             patch.object(manage, '_refresh', side_effect=failed), \
             patch.object(manage.os, 'replace', side_effect=no_catalog):
            with self.assertRaisesRegex(OSError, 'catalog write failure'):
                manage.refresh(self.root, import_plan_id=self.marker)
        self.assertEqual((self.root / manage.CATALOG).read_bytes(), original_catalog)
        self.assertEqual((self.root / manage.ORIGINS).read_bytes(), original_origins)

    def prepare_fixture(self):
        import manage
        members = []
        for row in self.rows:
            package = row['package']; directory = package['directory']; members.append(directory)
            self.write(directory + '/src/lib.rs', b'// current\n')
            manifest = f'[package]\nname = "{package["name"]}"\nversion = "{package["version"]}"\n'
            self.write(directory + '/Cargo.toml', manifest.encode())
            package['files_sha256']['Cargo.toml'] = digest(manifest.encode())
            row['source_disposition']['Cargo.toml']['current_paths'] = [directory + '/Cargo.toml']
            test_root = 'test/foundation/' + package['name']
            path = test_root + '/smoke.rs'; content = b'// local regression\n'
            self.write(path, content)
            row['external_tests'] = dict(directory=directory, origin_directory=package['origin_directory'],
                roots=[test_root], file_count=1, bytes=len(content),
                files=[dict(source=path, path=path, source_kind='project-authored',
                            source_sha256_before=digest(content), sha256=digest(content))])
        manifest = '[workspace]\nmembers = ' + json.dumps(members + ['src/app']) + '\ndefault-members = ["src/app"]\n'
        self.write('Cargo.toml', manifest.encode())
        self.write('src/app/Cargo.toml', b'[package]\nname="app"\nversion="0.1.0"\n')
        self.write('.cargo/config.toml', b'[net]\noffline=true\n[source.crates-io]\nreplace-with="empty"\n[source.empty]\ndirectory="empty-registry"\n')
        (self.root / 'empty-registry').mkdir()
        policy = dict(workspaces=[dict(manifest='Cargo.toml', targets=['x86_64-unknown-linux-gnu'])],
                      verification_members=members, integrated_sources=[])
        self.write(manage.POLICY, canonical(policy))
        origins = b'{"packages": []}'
        self.write(manage.ORIGINS, origins)
        previous = dict(packages={}, external_tests={}, origin_records_sha256={},
                        provenance_sha256={manage.ORIGINS: digest(origins)}, foundation_layout_receipt='d' * 64)
        raw = canonical(previous); self.write(manage.CATALOG, raw)
        self.plan['predecessor_catalog_sha256'] = digest(raw); self.pin()
        return previous, raw

    def test_prepare_success_preserves_predecessor_and_adds_exact_owners(self):
        import manage
        previous, raw = self.prepare_fixture()
        # This unit isolates import preparation; layout authentication is tested
        # by current() and the real full-catalog import/verify integration.
        with patch.object(manage.source_layout, 'load', return_value=None):
            candidate = imports.prepare(self.root, previous, self.marker, raw)
        self.assertEqual(previous['packages'], {})
        self.assertEqual(len(candidate['packages']), 3)
        self.assertEqual(len(candidate['external_tests']), 3)
        self.assertEqual(len(manage.origins(self.root)), 3)
        self.assertEqual((self.root / manage.CATALOG).read_bytes(), raw)

    def test_prepare_rejects_dormant_illegal_feature_before_origin_append(self):
        import manage
        previous, raw = self.prepare_fixture()
        package = self.rows[0]['package']; path = package['directory'] + '/Cargo.toml'
        data = (self.root / path).read_bytes() + b'[features]\nunused=["missing/feature"]\n'
        self.write(path, data); package['files_sha256']['Cargo.toml'] = digest(data); self.pin()
        with patch.object(manage.source_layout, 'load', return_value=None):
            with self.assertRaisesRegex(ValueError, 'dangling feature edge'):
                imports.prepare(self.root, previous, self.marker, raw)
        self.assertEqual(manage.origins(self.root), {})

    def test_external_upstream_hash_cannot_claim_another_original(self):
        row = self.rows[0]; directory = row['package']['directory']; path = 'test/foundation/wasm-text-parser/fragment.rs'
        row['external_tests']['files'] = [dict(source=directory + '/src/lib.rs', path=path,
            source_sha256_before='f' * 64, sha256=digest(b'fragment'))]
        row['source_disposition']['src/lib.rs']['current_paths'].append(path)
        self.pin()
        with self.assertRaisesRegex(ValueError, 'original attribution'):
            self.validate()

    def test_atomic_receipt_replacement_keeps_existing_mode(self):
        import manage
        import stat
        path = self.root / 'catalog.json'
        path.write_bytes(b'old')
        path.chmod(0o640)
        manage.atomic_write(path, b'new')
        self.assertEqual(path.read_bytes(), b'new')
        self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o640)

    def test_rollback_io_failure_reports_first_failure_and_incomplete_recovery(self):
        import manage
        from contextlib import redirect_stderr
        original_catalog = b'{"packages": {}}'
        original_origins = b'{"packages": []}'
        changed_origins = b'{"packages": ["new"]}'
        self.write(manage.CATALOG, original_catalog)
        self.write(manage.ORIGINS, original_origins)

        def prepared(root, previous, marker, raw):
            self.write(manage.ORIGINS, changed_origins)
            return previous

        real_write = manage.atomic_write
        def failed_recovery(path, data):
            if path == self.root / manage.ORIGINS and data == original_origins:
                raise OSError('rollback disk full')
            return real_write(path, data)

        stderr = io.StringIO()
        with patch.object(manage, 'ROOT', self.root), \
             patch.object(imports, 'prepare', side_effect=prepared), \
             patch.object(manage.source_layout, 'load', return_value=None), \
             patch.object(manage, '_refresh', side_effect=ValueError('first refresh failure')), \
             patch.object(manage, 'atomic_write', side_effect=failed_recovery), \
             patch.object(sys, 'argv', ['manage.py', 'refresh', '--import-plan', self.marker]), \
             redirect_stderr(stderr):
            self.assertEqual(manage.main(), 1)
        message = stderr.getvalue()
        self.assertIn('first refresh failure', message)
        self.assertIn('rollback disk full', message)
        self.assertIn('origins recovery is incomplete', message)
        self.assertEqual((self.root / manage.CATALOG).read_bytes(), original_catalog)
        self.assertEqual((self.root / manage.ORIGINS).read_bytes(), changed_origins)

    def test_new_error_module_is_explicitly_project_authored(self):
        row = self.rows[0]; directory = row['package']['directory']
        checksum = digest(b'// authored error enum')
        row['package']['files_sha256']['src/error.rs'] = checksum
        row['project_authored_files'][directory + '/src/error.rs'] = checksum
        self.pin()
        self.validate()
        row['project_authored_files'][directory + '/src/error.rs'] = 'f' * 64
        self.pin()
        with self.assertRaisesRegex(ValueError, 'project-authored production source mismatch'):
            self.validate()

    def test_stale_catalog(self):
        with self.assertRaisesRegex(ValueError, 'stale'):
            imports.prepare(self.root, {'foundation_layout_receipt': 'd' * 64}, self.marker, b'{}')


if __name__ == '__main__':
    unittest.main()
