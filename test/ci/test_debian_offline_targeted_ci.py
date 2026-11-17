"""Keep the offline source gate selected without replacing real SDK coverage."""
import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    'debian_offline_targeted_ci', ROOT / '.github/scripts/check-targeted-ci.py')
CI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CI)


class DebianOfflineSelection(unittest.TestCase):
    def test_every_entrypoint_and_test_path_selects_the_fake_command_proofs(self):
        policy = json.loads((ROOT / CI.PLAN_PATH).read_text())
        for path in ('.github/scripts/debian13-run.sh', '.github/scripts/smb-sdk.sh',
                     'test/ci/test_debian_offline_runner.py',
                     'test/ci/test_smb_build_only.py',
                     'test/ci/test_debian_offline_targeted_ci.py'):
            with self.subTest(path=path):
                plan = CI.select_plan(policy, [path], shardable=True)
                self.assertIn('debian-offline-entrypoint', plan['groups'])
                self.assertTrue({'test_debian_offline_runner', 'test_smb_build_only',
                                 'test_debian_offline_targeted_ci'} <= set(plan['python_tests']))
                if path.endswith('/smb-sdk.sh'):
                    self.assertIn('smb-vfs-manual-fd', plan['groups'])

    def test_offline_fake_tools_do_not_claim_compilation_or_installation(self):
        policy = json.loads((ROOT / CI.PLAN_PATH).read_text())
        group = policy['groups']['debian-offline-entrypoint']
        self.assertEqual(group['rust_targets'], [])
        plan = CI.select_plan(policy, ['src/web/src/monitor/history.rs'], shardable=True)
        self.assertNotIn('debian-offline-entrypoint', plan['groups'])


if __name__ == '__main__':
    unittest.main()
