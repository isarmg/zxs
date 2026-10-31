"""Execute both shell branches using controlled fake toolchain commands; no SDK build."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
FAKE = r'''#!/usr/bin/env python3
import json, os, pathlib, sys
name=pathlib.Path(sys.argv[0]).name
with open(os.environ['CALLS'], 'a') as f: f.write(json.dumps([name,*sys.argv[1:]])+'\n')
if name=='dpkg': print('amd64')
elif name=='dpkg-query': print('fixture-version')
elif name=='smbd': print('Version fixture')
elif name=='git': print(('b' if sys.argv[-1]=='HEAD^{tree}' else 'a')*40)
elif name=='ldd': print('libc.so.6 => '+('not found' if os.environ.get('MISSING_LIB') else '/lib/libc.so.6 (0x123)'))
elif name=='waf':
    p=pathlib.Path('bin/default/include/config.h');p.parent.mkdir(parents=True,exist_ok=True);p.write_text('fixture generated header')
elif name=='cargo':
    if sys.argv[1]=='rustc':
        print('ZXS SMB VFS requires panic=unwind to isolate panics at the host boundary')
        sys.exit(1)
    if sys.argv[1]=='build':
        p=pathlib.Path(os.environ['CARGO_TARGET_DIR'])/'release/libsmb_vfs.so'
        p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(b'fake module, not a runtime proof')
    if 'storage-server' in sys.argv:
        print(json.dumps({'reason':'compiler-artifact','executable':'/fixture/test-binary','target':{'name':'storage_server'}}))
'''


class SmbBuildOnly(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        scripts = self.root / '.github/scripts'
        scripts.mkdir(parents=True)
        self.os_release = self.root / 'os-release'
        self.os_release.write_text('ID=debian\nVERSION_ID=13\n')
        for name in ('smb-sdk.sh', 'smb-vfs-check.sh'):
            script = (ROOT / '.github/scripts' / name).read_text()
            # Only relocate the OS fixture file; no production bypass setting.
            (scripts / name).write_text(script.replace('/etc/os-release', str(self.os_release)))
        (scripts / 'check-rust-diagnostics.py').write_text('import subprocess,sys\nsys.exit(subprocess.run(sys.argv[2:]).returncode)\n')
        source = self.root / 'target/smb-debian13/source'
        source.mkdir(parents=True)
        (source / '.downloaded').touch()
        (source / 'samba_fixture.dsc').write_bytes(b'fixed Debian source bytes')
        target = {'package_version': 'fixture-version', 'source_files': {'samba_fixture.dsc': hashlib.sha256(b'fixed Debian source bytes').hexdigest()}}
        (self.root / '.github/samba-debian-target.json').write_text(json.dumps(target))
        self.sdk = source / 'sdk'
        (self.sdk / 'buildtools/bin').mkdir(parents=True)
        self.bin = self.root / 'fake-bin'
        self.bin.mkdir()
        for name in ('dpkg', 'dpkg-query', 'apt-get', 'dpkg-source', 'make', 'cargo', 'git', 'smbd', 'readelf', 'ldd'):
            executable = self.bin / name
            executable.write_text(FAKE)
            executable.chmod(0o755)
        waf = self.sdk / 'buildtools/bin/waf'
        waf.write_text(FAKE)
        waf.chmod(0o755)
        self.calls = self.root / 'calls.jsonl'
        self.env = dict(os.environ, PATH=str(self.bin) + ':' + os.environ['PATH'], CALLS=str(self.calls),
                        CARGO_TARGET_DIR=str(self.root / 'cargo-target'))

    def run_script(self, *args, script='smb-sdk.sh'):
        return subprocess.run(['bash', str(self.root / '.github/scripts' / script), *args],
                              cwd=self.root, env=self.env, capture_output=True, text=True)

    def recorded(self):
        return [json.loads(line) for line in self.calls.read_text().splitlines()] if self.calls.exists() else []

    def test_full_default_keeps_all_validation_and_fixture_branches(self):
        result = self.run_script()
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.recorded()
        self.assertTrue(any(c[:2] == ['cargo', 'clippy'] for c in calls))
        self.assertTrue(any(c[:2] == ['cargo', 'rustc'] for c in calls))
        self.assertTrue(any(c[:2] == ['cargo', 'test'] and 'smb-vfs' in c for c in calls))
        self.assertTrue(any('storage-server' in c for c in calls))
        receipt = json.loads((self.root / 'target/smb-debian13/product/VFS-BUILD.json').read_text())
        self.assertEqual(receipt['validation_mode'], 'full')
        self.assertEqual(receipt['runtime_acceptance'], 'not-run-by-build-script')

    def test_build_only_still_prepares_sdk_and_links_exact_release_module(self):
        result = self.run_script('--build-only')
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.recorded()
        self.assertTrue(any(c[0] == 'make' for c in calls))
        self.assertTrue(any(c[0] == 'waf' for c in calls))
        cargo = [c for c in calls if c[0] == 'cargo']
        self.assertEqual(len(cargo), 1)
        self.assertEqual(cargo[0][1], 'build')
        self.assertIn('--release', cargo[0])
        self.assertTrue(any(c[0] == 'readelf' for c in calls))
        self.assertTrue(any(c[0] == 'ldd' for c in calls))
        product = self.root / 'target/smb-debian13/product'
        receipt = json.loads((product / 'VFS-BUILD.json').read_text())
        self.assertEqual(receipt['validation_mode'], 'build-only')
        self.assertEqual(receipt['commit'], 'a' * 40)
        self.assertEqual(receipt['tree'], 'b' * 40)
        self.assertEqual(receipt['module_sha256'], hashlib.sha256((product / 'zxs.so').read_bytes()).hexdigest())
        self.assertEqual(receipt['sdk_config_sha256'], hashlib.sha256((product.parent / 'source/sdk/bin/default/include/config.h').read_bytes()).hexdigest())
        self.assertFalse((product.parent / 'fixture-path.txt').exists())

    def test_valid_cache_reuses_sdk_but_changed_configuration_rebuilds(self):
        self.assertEqual(self.run_script('--build-only').returncode, 0)
        self.calls.write_text('')
        self.assertEqual(self.run_script('--build-only').returncode, 0)
        self.assertFalse(any(c[0] in ('make', 'waf') for c in self.recorded()))
        target = self.root / '.github/samba-debian-target.json'
        metadata = json.loads(target.read_text())
        metadata['configuration_revision'] = 'changed-fixture'
        target.write_text(json.dumps(metadata))
        self.calls.write_text('')
        self.assertEqual(self.run_script('--build-only').returncode, 0)
        self.assertTrue(any(c[0] == 'make' for c in self.recorded()))
        self.assertTrue(any(c[0] == 'waf' for c in self.recorded()))
        (self.sdk / 'bin/default/include/config.h').unlink()
        self.calls.write_text('')
        self.assertEqual(self.run_script('--build-only').returncode, 0)
        self.assertTrue(any(c[0] == 'waf' for c in self.recorded()))

    def test_unknown_or_multiple_arguments_execute_no_dependencies(self):
        for script in ('smb-sdk.sh', 'smb-vfs-check.sh'):
            for args in (('--unknown',), ('--build-only', '--build-only'), ('full',)):
                with self.subTest(script=script, args=args):
                    result = self.run_script(*args, script=script)
                    self.assertEqual(result.returncode, 2)
                    self.assertEqual(self.recorded(), [])

    def test_cached_source_tampering_is_rejected_before_any_compilation(self):
        (self.sdk.parent / 'samba_fixture.dsc').write_bytes(b'tampered')
        result = self.run_script('--build-only')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Debian source checksum mismatch', result.stderr)
        self.assertFalse(any(c[0] in ('make', 'waf', 'cargo') for c in self.recorded()))

    def test_missing_marker_requires_download_and_source_extraction(self):
        (self.sdk.parent / '.downloaded').unlink()
        result = self.run_script('--build-only')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(any(c[0] == 'apt-get' for c in self.recorded()))
        self.assertTrue(any(c[0] == 'dpkg-source' for c in self.recorded()))

    def test_build_only_does_not_skip_missing_runtime_library_failure(self):
        self.env['MISSING_LIB'] = '1'
        result = self.run_script('--build-only')
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / 'target/smb-debian13/product/VFS-BUILD.json').exists())


if __name__ == '__main__':
    unittest.main()
