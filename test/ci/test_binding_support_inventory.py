"""Keep the original logical inventory and independent WAT expectations visible."""
from collections import Counter
import hashlib
import json
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]


class BindingSupportInventory(unittest.TestCase):
    def test_original_cases_and_fixture_bytes_are_accounted_for(self):
        inventory = json.loads((ROOT / 'test/ci/fixtures/binding-support-tests.json').read_text())
        self.assertEqual(len(inventory['original_logical_cases']), 87)
        self.assertEqual(len(set(inventory['original_logical_cases'])), 87)
        self.assertEqual(len(inventory['retired_positive_cases']), 1)
        retired = inventory['retired_positive_cases'][0]
        self.assertEqual(retired['name'], 'interpreter::smoke_tests::invoke_redirect')
        self.assertIn(retired['name'], inventory['original_logical_cases'])
        base = ROOT / 'test/foundation/wasm-bindgen-cli-support'
        actual = {p.relative_to(ROOT).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
                  for p in base.rglob('*.wat')}
        self.assertEqual(len(actual), 19)
        self.assertEqual(actual, inventory['wat_fixtures'])
        source = '\n'.join(p.read_text() for p in base.rglob('*.rs'))
        names = re.findall(r'#\[test\]\s*(?:#\[should_panic\]\s*)?fn\s+(\w+)\s*\(', source)
        self.assertEqual(len(names), 87)
        expected = [name.rsplit('::', 1)[-1] for name in inventory['original_logical_cases']
                    if name != retired['name']] + [retired['replacement'].rsplit('::', 1)[-1]]
        self.assertEqual(Counter(names), Counter(expected))
        self.assertNotIn('#[ignore', source)
        self.assertIn(retired['replacement'].rsplit('::', 1)[1], names)
        self.assertNotIn('rstest::', source)
        self.assertNotIn('option_env!("BLESS")', source)
        self.assertNotIn('fs::write(', source)

    def test_full_lib_is_an_actual_foundation_command(self):
        commands = json.loads((ROOT / 'test/ci/foundation-commands.json').read_text())
        self.assertIn({'tool': 'cargo', 'args': ['test', '--locked', '--offline', '-p',
                      'wasm-bindgen-cli-support', '--lib']}, commands['foundation-language'])


if __name__ == '__main__':
    unittest.main()
