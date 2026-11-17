"""Exercise offline runner admission with fake Docker/Rust, never a real build."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
IMAGE_ID = "sha256:" + "a" * 64
FAKE = r'''#!/usr/bin/env python3
import json, os, pathlib, sys
name = pathlib.Path(sys.argv[0]).name
with open(os.environ['CALLS'], 'a') as output:
    output.write(json.dumps([name, *sys.argv[1:]]) + '\n')
if name == 'rustc':
    print(os.environ['ZXS_BUILD_TOOLCHAIN_ROOT'])
elif sys.argv[1:3] == ['image', 'inspect']:
    if os.environ.get('MISSING_IMAGE'):
        sys.exit(1)
    print(os.environ['IMAGE_ID'])
elif sys.argv[1] == 'run':
    sys.exit(int(os.environ.get('CONTAINER_STATUS', '0')))
elif sys.argv[1] != 'build':
    sys.exit(17)
'''


class DebianOfflineRunner(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        scripts = self.root / '.github/scripts'
        scripts.mkdir(parents=True)
        self.script = scripts / 'debian13-run.sh'
        self.script.write_text((ROOT / '.github/scripts/debian13-run.sh').read_text())
        (self.root / '.github/samba-debian-target.json').write_text(
            json.dumps({'package_version': 'fixture-version'}))
        self.toolchain = self.root / 'complete toolchain'
        (self.toolchain / 'bin').mkdir(parents=True)
        for name in ('rustc', 'cargo'):
            executable = self.toolchain / 'bin' / name
            executable.write_text('#!/bin/sh\nexit 81\n')
            executable.chmod(0o700)
        self.bin = self.root / 'fake-bin'
        self.bin.mkdir()
        for name in ('docker', 'rustc'):
            executable = self.bin / name
            executable.write_text(FAKE)
            executable.chmod(0o700)
        self.calls = self.root / 'calls.jsonl'
        self.env = dict(os.environ, PATH=f"{self.bin}{os.pathsep}{os.environ['PATH']}",
                        CALLS=str(self.calls), IMAGE_ID=IMAGE_ID,
                        ZXS_BUILD_NETWORK='offline',
                        ZXS_BUILD_TOOLCHAIN_ROOT=str(self.toolchain))

    def run_script(self, *args, **environment):
        return subprocess.run(['bash', str(self.script), *args], cwd='/tmp',
                              env=dict(self.env, **environment), capture_output=True, text=True)

    def recorded(self):
        return [json.loads(line) for line in self.calls.read_text().splitlines()] \
            if self.calls.exists() else []

    def test_offline_pins_local_image_without_build_pull_or_host_rustc(self):
        result = self.run_script('--native', 'printf', '%s', 'one complete argument')
        self.assertEqual(result.returncode, 0, result.stderr)
        inspect, run = self.recorded()
        self.assertEqual(inspect, ['docker', 'image', 'inspect', '--format',
                                   '{{.Id}}', 'zxs-debian13-native'])
        self.assertEqual(run[:4], ['docker', 'run', '--rm', '--init'])
        self.assertIn('--pull=never', run)
        self.assertEqual(run[run.index('--network') + 1], 'none')
        self.assertIn('ZXS_BUILD_NETWORK=offline', run)
        self.assertIn(f'{self.toolchain}:/opt/rust-toolchain:ro', run)
        self.assertEqual(run[-4:], [IMAGE_ID, 'printf', '%s', 'one complete argument'])
        self.assertNotIn('zxs-debian13-native', run)

    def test_browser_keeps_browser_environment_and_one_network_boundary(self):
        result = self.run_script('--browser', 'fixture-command')
        self.assertEqual(result.returncode, 0, result.stderr)
        inspect, run = self.recorded()
        self.assertEqual(inspect[-1], 'zxs-debian13-browser')
        self.assertEqual(run.count('--network'), 1)
        self.assertIn('CHROME_BIN=/usr/bin/chromium', run)
        self.assertIn('CHROMEDRIVER=/usr/bin/chromedriver', run)
        self.assertIn('--user', run)

    def test_missing_or_ambiguous_image_never_falls_back_to_build_or_pull(self):
        for environment in ({'MISSING_IMAGE': '1'}, {'IMAGE_ID': 'mutable-tag'},
                            {'IMAGE_ID': IMAGE_ID + '\n' + IMAGE_ID}):
            with self.subTest(environment=environment):
                self.calls.write_text('')
                result = self.run_script('fixture-command', **environment)
                self.assertNotEqual(result.returncode, 0)
                calls = self.recorded()
                self.assertEqual(len(calls), 1)
                self.assertEqual(calls[0][:3], ['docker', 'image', 'inspect'])

    def test_missing_toolchain_and_unsafe_volume_paths_run_no_dependencies(self):
        for value in ('', 'relative', '/does-not-exist', str(self.toolchain) + ':rw',
                      str(self.toolchain) + '\n'):
            with self.subTest(value=value):
                result = self.run_script('fixture-command', ZXS_BUILD_TOOLCHAIN_ROOT=value)
                self.assertEqual(result.returncode, 2)
                self.assertEqual(self.recorded(), [])
        (self.toolchain / 'bin/cargo').unlink()
        result = self.run_script('fixture-command')
        self.assertEqual(result.returncode, 2)
        self.assertEqual(self.recorded(), [])

    def test_unknown_policy_fails_before_any_dependencies(self):
        result = self.run_script('fixture-command', ZXS_BUILD_NETWORK='offlien')
        self.assertEqual(result.returncode, 2)
        self.assertEqual(self.recorded(), [])

    def test_container_failure_is_not_replaced_by_an_online_retry(self):
        result = self.run_script('fixture-command', CONTAINER_STATUS='29')
        self.assertEqual(result.returncode, 29)
        self.assertEqual(len(self.recorded()), 2)

    def test_default_online_path_keeps_preparing_its_existing_image(self):
        environment = dict(self.env)
        del environment['ZXS_BUILD_NETWORK']
        result = subprocess.run(['bash', str(self.script), 'fixture-command'],
                                env=environment, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        build, rustc, run = self.recorded()
        self.assertEqual(build[:2], ['docker', 'build'])
        self.assertIn('SAMBA_VERSION=fixture-version', build)
        self.assertEqual(rustc, ['rustc', '--print', 'sysroot'])
        self.assertEqual(run[-2:], ['zxs-debian13-build', 'fixture-command'])
        self.assertIn('ZXS_BUILD_NETWORK=online', run)


if __name__ == '__main__':
    unittest.main()
