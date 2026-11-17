"""Real-SDK routing and current-inventory requirements, not a VFS runtime substitute."""
import importlib.util
import json
from pathlib import Path
import unittest

from test_targeted_ci import CI

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    'smb_manual_descriptors', ROOT / '.github/scripts/check-smb-manual-descriptors.py')
SDK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SDK)


class SmbManualDescriptorSelection(unittest.TestCase):
    def test_descriptor_paths_require_sdk_without_a_plain_ubuntu_vfs_build(self):
        policy = json.loads((ROOT / '.github/ci/targeted-tests.json').read_text())
        for path in (
            'src/smb-vfs/src/module.rs',
            'src/smb-vfs/src/module/descriptors.rs',
            'test/smb-vfs/unit/module/tests.rs',
            'test/smb-vfs/unit/module/manual_fd_tests.rs',
            'test/smb-vfs/unit/module/mtime_tests.rs',
            'test/smb-vfs/unit/module/seek_tests.rs',
            '.github/scripts/check-smb-manual-descriptors.py',
            '.github/scripts/smb-vfs-check.sh',
            '.github/scripts/smb-sdk.sh',
            '.github/workflows/smb-sdk.yml',
            'test/ci/test_smb_vfs_manual_ci.py',
        ):
            with self.subTest(path=path):
                plan = CI.select_plan(policy, [path], shardable=True)
                self.assertIn(SDK.GROUP, plan['groups'])
                self.assertIn('test_smb_vfs_manual_ci', plan['python_tests'])
                self.assertFalse(any(t['package'] == 'smb-vfs' for t in plan['rust_targets']))

    def test_eight_exact_current_tests_are_required_and_never_satisfied_by_zero(self):
        self.assertEqual(len(SDK.EXACT), 8)
        self.assertEqual(len(set(SDK.EXACT)), 8)
        self.assertEqual(SDK.TARGET['features'], ['smb-sdk'])
        self.assertEqual(CI.selected_tests(SDK.EXACT, [], SDK.TARGET), sorted(SDK.EXACT))
        with self.assertRaises(ValueError):
            CI.selected_tests(SDK.EXACT[1:], [], SDK.TARGET)
        with self.assertRaises(ValueError):
            CI.selected_tests(SDK.EXACT, [SDK.EXACT[0]], SDK.TARGET)
        with self.assertRaises(ValueError):
            CI.selected_tests([], [], SDK.TARGET)

    def test_push_selection_and_coverage_require_the_separate_sdk_job(self):
        workflow = (ROOT / '.github/workflows/ci.yml').read_text()
        self.assertIn('branches: [main]', workflow)
        self.assertIn("summary['head'] != os.environ['GITHUB_SHA']", workflow)
        self.assertIn("selected = 'smb-vfs-manual-fd' in summary['plan']['groups']", workflow)
        self.assertIn('smb_manual_fd: ${{ steps.sdk.outputs.smb_manual_fd }}', workflow)
        self.assertIn('needs: [targeted-plan, targeted, targeted-smb-sdk]', workflow)
        self.assertIn('uses: ./.github/workflows/smb-sdk.yml', workflow)
        self.assertIn('suite: manual-fd', workflow)
        self.assertIn('run: test "$ZXS_TARGETED_SDK_RESULT" = success', workflow)

    def test_narrow_scope_cannot_claim_full_sdk_or_system_samba_acceptance(self):
        workflow = (ROOT / '.github/workflows/smb-sdk.yml').read_text()
        self.assertIn('default: full', workflow)
        self.assertIn("if: inputs.suite == 'full'", workflow)
        self.assertIn('name: debian13-vfs-manual-fd-${{ github.sha }}', workflow)
        self.assertIn('target/targeted-ci/summary.json', workflow)
        self.assertIn("receipt['head'] != os.environ['ZXS_EXPECTED_HEAD']", workflow)
        self.assertIn("receipt['status'] != 'selected-sdk-tests-passed'", workflow)
        sdk = (ROOT / '.github/scripts/smb-sdk.sh').read_text()
        branch = sdk.split('if [[ "$mode" == manual-fd-tests ]]; then', 1)[1].split('fi', 1)[0]
        self.assertIn('bash .github/scripts/smb-vfs-check.sh --manual-fd-tests', branch)
        self.assertIn('exit 0', branch)
        runner = (ROOT / '.github/scripts/check-smb-manual-descriptors.py').read_text()
        self.assertIn('run.rust(TARGET)', runner)
        self.assertIn("('failed', 'skipped', 'blocked')", runner)
        self.assertIn('sorted(EXACT)', runner)
