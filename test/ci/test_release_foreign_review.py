"""Execute the acceptance shell with a Cargo observer, not Chrome or an ELF."""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SWITCH = 'ZXS_SMOKE_FOREIGN_REVIEW'
VERSION = '1.2.3-review.1'
ARTIFACT = b'not an ELF: acceptance invocation fixture\n'
CARGO_ARGS = ['run', '--locked', '--offline', '--release', '-p', 'browser-smoke', '--']

# This records the real script's copied input and process arguments. It does not
# pretend to serve Web assets, authenticate, exercise a browser or import data.
CARGO_OBSERVER = '''
import hashlib
import json
import os
from pathlib import Path
import stat
import sys

args = sys.argv[1:]
binary = Path(args[args.index('--cli-bin') + 1])
log = Path(os.environ['MOCK_CARGO_LOG'])
call = len(log.read_text().splitlines()) + 1
identity = binary.stat()
record = {
    'args': args, 'cwd': os.getcwd(), 'binary': str(binary),
    'sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
    'dev': identity.st_dev, 'ino': identity.st_ino,
    'mode': stat.S_IMODE(identity.st_mode),
    'siblings': sorted(path.name for path in binary.parent.iterdir()),
    'work_mode': stat.S_IMODE(binary.parent.parent.stat().st_mode),
}
with log.open('a') as output:
    output.write(json.dumps(record) + '\\n')
if call == int(os.environ['MOCK_FAIL_CALL']):
    sys.exit(int(os.environ['MOCK_EXIT_CODE']))
'''


class ReleaseForeignReviewExecution(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='release foreign review ')
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.script = self.root / '.github/scripts/release-acceptance.sh'
        self.script.parent.mkdir(parents=True)
        shutil.copyfile(ROOT / '.github/scripts/release-acceptance.sh', self.script)
        self.asset = self.root / f'release-assets/zxs-{VERSION}-debian13-amd64'
        self.asset.parent.mkdir()
        self.asset.write_bytes(ARTIFACT)
        self.asset.chmod(0o644)
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        cargo = self.bin / 'cargo'
        cargo.write_text(f'#!{sys.executable}\n' + CARGO_OBSERVER)
        cargo.chmod(0o755)
        self.scratch = self.root / 'temporary work'
        self.scratch.mkdir()
        self.log = self.root / 'calls.jsonl'

    def invoke(self, *, switch=None, screenshot=None, fail_call=0, exit_code=47,
               version=VERSION):
        env = os.environ.copy()
        for name in (SWITCH, 'ZXS_SMOKE_SCREENSHOT', 'RELEASE_VERSION'):
            env.pop(name, None)
        env.update({
            'PATH': f'{self.bin}{os.pathsep}{env["PATH"]}',
            'TMPDIR': str(self.scratch),
            'MOCK_CARGO_LOG': str(self.log),
            'MOCK_FAIL_CALL': str(fail_call),
            'MOCK_EXIT_CODE': str(exit_code),
        })
        if version is not None:
            env['RELEASE_VERSION'] = version
        if switch is not None:
            env[SWITCH] = switch
        if screenshot is not None:
            env['ZXS_SMOKE_SCREENSHOT'] = screenshot
        self.log.write_text('')
        result = subprocess.run(['bash', str(self.script)], cwd=self.root,
                                env=env, capture_output=True, text=True, timeout=10)
        calls = [json.loads(line) for line in self.log.read_text().splitlines()]
        self.assertEqual(list(self.scratch.iterdir()), [], 'private copy was not cleaned')
        self.assertEqual(self.asset.read_bytes(), ARTIFACT)
        self.assertEqual(self.asset.stat().st_mode & 0o777, 0o644)
        for call in calls:
            self.assertEqual(call['sha256'], hashlib.sha256(ARTIFACT).hexdigest())
            self.assertEqual(call['mode'], 0o755)
            self.assertEqual(call['work_mode'], 0o700)
            self.assertEqual(call['siblings'], ['zxs'])
            self.assertEqual(call['cwd'], str(self.root))
            self.assertNotEqual(call['binary'], str(self.asset))
            self.assertFalse(Path(call['binary']).exists())
        return result, calls

    def assert_call(self, call, extra=()):
        expected = CARGO_ARGS + ['--cli-bin', call['binary'], '--embedded-web',
                                 '--server-container-image', 'zxs-debian13-runtime']
        self.assertEqual(call['args'], expected + list(extra))

    def test_unset_switch_keeps_one_full_invocation(self):
        result, calls = self.invoke()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(calls), 1)
        self.assert_call(calls[0])

    def test_zero_keeps_one_full_invocation(self):
        result, calls = self.invoke(switch='0')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(calls), 1)
        self.assert_call(calls[0])

    def test_one_runs_full_then_foreign_with_same_private_elf(self):
        result, calls = self.invoke(switch='1')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(calls), 2)
        self.assert_call(calls[0])
        self.assert_call(calls[1], ['--foreign-review-only'])
        for field in ('binary', 'sha256', 'dev', 'ino'):
            self.assertEqual(calls[0][field], calls[1][field])

    def test_screenshot_with_spaces_is_only_given_to_full_invocation(self):
        path = 'artifacts/a screenshot with spaces.png'
        result, calls = self.invoke(switch='1', screenshot=path)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(calls), 2)
        self.assert_call(calls[0], ['--screenshot', path])
        self.assert_call(calls[1], ['--foreign-review-only'])

    def test_full_failure_stops_before_foreign_and_preserves_exit_code(self):
        result, calls = self.invoke(switch='1', fail_call=1, exit_code=43)
        self.assertEqual(result.returncode, 43)
        self.assertEqual(len(calls), 1)
        self.assert_call(calls[0])

    def test_foreign_failure_preserves_failure_and_cleans_private_elf(self):
        result, calls = self.invoke(switch='1', fail_call=2, exit_code=47)
        self.assertEqual(result.returncode, 47)
        self.assertEqual(len(calls), 2)
        self.assert_call(calls[0])
        self.assert_call(calls[1], ['--foreign-review-only'])

    def test_switch_rejects_empty_and_other_values_before_any_invocation(self):
        for value in ('', 'true', 'false', '2', '01', ' 1', '1\n', '1; exit 0'):
            with self.subTest(value=value):
                result, calls = self.invoke(switch=value)
                self.assertEqual(result.returncode, 1)
                self.assertIn(f'{SWITCH} must be 0 or 1', result.stderr)
                self.assertEqual(calls, [])

    def test_release_version_validation_still_precedes_any_invocation(self):
        for version in (None, '', '../other', 'not-a-version'):
            with self.subTest(version=version):
                result, calls = self.invoke(switch='1', version=version)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(calls, [])


class ReleaseForeignReviewWorkflow(unittest.TestCase):
    def setUp(self):
        self.ci = (ROOT / '.github/workflows/ci.yml').read_text()
        self.release = (ROOT / '.github/workflows/release.yml').read_text()
        self.browser = self.ci.split('  browser-smoke:\n', 1)[1].split('  smb-sdk:', 1)[0]

    def test_only_ci_acceptance_step_explicitly_enables_extra_mode(self):
        step = self.browser.split('      - name: Build and exercise extracted release packages\n', 1)[1]
        step = step.split('      - name:', 1)[0]
        self.assertEqual(self.ci.count(SWITCH), 1)
        self.assertIn(f"          {SWITCH}: '1'\n", step)
        self.assertNotIn(SWITCH, self.release)
        self.assertIn('bash .github/scripts/release-acceptance.sh', self.release)
        for later in ('check-csr-browser.py', 'release-assemble.sh'):
            self.assertLess(step.index('release-acceptance.sh'), step.index(later))
        self.assertNotIn('continue-on-error:', self.browser)
        self.assertEqual(self.browser.count('release-acceptance.sh'), 1)

    def test_existing_dependencies_and_both_concurrency_policies_are_unchanged(self):
        self.assertIn('    needs: [static, web-production, smb-sdk]\n', self.browser)
        self.assertIn('    timeout-minutes: 60\n', self.browser)
        expected = (
            (self.ci, '  group: ci-${{ github.workflow }}-${{ github.event.pull_request.number || github.ref }}\n'
             '  cancel-in-progress: true\n'),
            (self.release, '  group: release-${{ github.ref }}\n'
             '  cancel-in-progress: false\n'),
        )
        for source, policy in expected:
            self.assertEqual(re.search(r'^concurrency:\n((?:  .+\n)+)', source,
                                       re.MULTILINE).group(1), policy)

    def test_foreign_evidence_is_scoped_and_attempted_even_after_failure(self):
        block = self.browser.split('      - name: Retain foreign review browser evidence\n', 1)[1]
        self.assertIn('        if: always()\n', block)
        self.assertIn('actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9', block)
        self.assertIn('          name: foreign-review-browser-${{ github.sha }}\n', block)
        self.assertIn('          path: artifacts/browser-smoke/foreign-review/\n', block)
        self.assertIn('          if-no-files-found: warn\n', block)
        self.assertIn('          retention-days: 7\n', block)
        self.assertNotIn('overwrite:', block)
        self.assertIn('          path: artifacts/browser-smoke/placement/\n', self.browser)
        self.assertIn('          name: installation-candidate-linux-web\n', self.browser)


if __name__ == '__main__':
    unittest.main()
