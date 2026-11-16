"""Select real local SMB RPC directory and content tests without calling them Samba."""
import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    'targeted_ci', ROOT / '.github/scripts/check-targeted-ci.py',
)
CI = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CI)


class NativeSmbMkdirSelection(unittest.TestCase):
    def test_default_share_selects_real_native_mkdir_and_copy_checks(self):
        policy = json.loads((ROOT / '.github/ci/targeted-tests.json').read_text())
        plan = CI.select_plan(policy, ['src/server/src/smb_rpc/default_share.rs'])
        self.assertIn('ordinary-smb-native-mkdir', plan['groups'])
        targets = plan['rust_targets']
        tests = [target for target in targets if target.get('mode', 'test') == 'test']
        self.assertCountEqual([target['max_tests'] for target in tests], [3, 1])
        self.assertTrue(all(target['isolate'] and target['features'] == ['core']
                            for target in tests))
        self.assertTrue(any(
            target.get('name') == 'phase1_smb_rpc'
            and target['exact'] == [
                'public_smb_bridge_read_to_new_file_keeps_independent_history_after_restart',
            ]
            for target in tests
        ))
        self.assertTrue(any(
            target['kind'] == 'lib'
            and 'smb_rpc::tests::default_share_tests::'
                'default_mkdir_replay_admission_precedes_any_internal_root_creation'
                in target['exact']
            for target in tests
        ))
