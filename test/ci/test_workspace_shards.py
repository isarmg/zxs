"""Complete workspace partition proofs; no compiler, native product or network."""
from collections import Counter
import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('workspace_shards', ROOT / '.github/scripts/check-workspace-shards.py')
GATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GATE)
POLICY_SPEC = importlib.util.spec_from_file_location('workspace_policy', ROOT / '.github/scripts/development-ci.py')
POLICY = importlib.util.module_from_spec(POLICY_SPEC)
POLICY_SPEC.loader.exec_module(POLICY)


class WorkspaceShards(unittest.TestCase):
    def inventory(self):
        return [{'id': f'member/{target}:lib:example', 'manifest': f'member/{target}/Cargo.toml',
                 'tests': [{'name': f'case_{number}', 'kind': 'test', 'ignored': number == 1}
                           for number in range(80)]} for target in ('a', 'b')]

    def receipts(self):
        inventory = self.inventory()
        assignment = GATE.allocate(inventory, {})
        receipts = []
        for shard in GATE.SHARDS:
            receipt = {'schema': 1, 'shard': shard, 'head': 'exact-head',
                       'lockfile_sha256': 'same-lock', 'status': 'success',
                       'commands': [{'command': ['git', 'rev-parse', 'HEAD'], 'returncode': 0}], 'results': []}
            if shard.startswith('tests-'):
                index = int(shard.split('-')[1])
                receipt.update(inventory=copy.deepcopy(inventory), inventory_sha256=GATE.digest(inventory), weights_sha256=GATE.digest({}))
                for target in inventory:
                    selected = [t['name'] for t in target['tests'] if assignment[(target['id'], t['name'])] == index]
                    if selected:
                        ignored = sum(t['ignored'] for t in target['tests'] if t['name'] in selected)
                        receipt['results'].append({'target': target['id'], 'assigned': selected, 'status': 'success',
                                                   'counts': {'passed': len(selected)-ignored, 'failed': 0, 'ignored': ignored,
                                                              'measured': 0, 'filtered': len(target['tests'])-len(selected)}})
            else:
                receipt['commands'] += [{'command': command, 'returncode': 0} for command in GATE.auxiliary_commands(shard)]
                if shard == 'ignored':
                    receipt['results'] = [{'name': name, 'status': 'success', 'counts':
                        {'passed': 1, 'failed': 0, 'ignored': 0, 'measured': 0, 'filtered': 99}} for name in GATE.IGNORED]
            receipts.append(receipt)
        return receipts

    def verify(self, receipts):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for index, receipt in enumerate(receipts):
                (root / str(index)).mkdir()
                (root / str(index) / 'receipt.json').write_text(json.dumps(receipt))
            GATE.verify(root, 'exact-head', {})

    def test_all_target_name_pairs_are_assigned_exactly_once(self):
        inventory = self.inventory()
        assignment = GATE.allocate(inventory, {})
        expected = {(t['id'], c['name']) for t in inventory for c in t['tests']}
        self.assertEqual(set(assignment), expected)
        self.assertEqual(set(assignment.values()), set(range(8)))
        self.assertEqual(assignment, GATE.allocate(list(reversed(inventory)), {}))
        # Same name in two different binaries is two distinct tests.
        self.assertEqual(len(assignment), 160)

    def test_known_expensive_tests_are_separated_without_excluding_new_tests(self):
        inventory = self.inventory()[:1]
        weights = {f'case_{i}': 10000-i for i in range(8)}
        assignment = GATE.allocate(inventory, weights)
        identity = inventory[0]['id']
        self.assertEqual({assignment[(identity, f'case_{i}')] for i in range(8)}, set(range(8)))
        self.assertEqual(len(assignment), 80)

    def test_duplicate_identity_is_rejected(self):
        inventory = self.inventory()
        inventory.append(copy.deepcopy(inventory[0]))
        with self.assertRaisesRegex(ValueError, 'duplicate'):
            GATE.allocate(inventory, {})

    def test_list_parser_preserves_ignored_query_and_benchmark_names(self):
        self.assertEqual(GATE.listed('a: test\nb: benchmark\n\n1 test, 1 benchmark\n'), [('a', 'test'), ('b', 'benchmark')])
        self.assertEqual(GATE.listed('0 tests, 0 benchmarks\n'), [])
        for output in ('a: test\na: test\n', 'warning: custom harness\n'):
            with self.assertRaises(ValueError):
                GATE.listed(output)

    def test_summary_requires_assigned_count_and_ignored_inventory(self):
        summary = 'test result: ok. 2 passed; 0 failed; 1 ignored; 0 measured; 7 filtered out; finished in 1s\n'
        self.assertEqual(GATE.validate_result(summary, 3, 10, 0, 1)['passed'], 2)
        for selected, total, code, ignored in ((2, 10, 0, 1), (3, 9, 0, 1), (3, 10, 1, 1), (3, 10, 0, 0)):
            with self.subTest(selected=selected, total=total, code=code, ignored=ignored), self.assertRaises(ValueError):
                GATE.validate_result(summary, selected, total, code, ignored)
        for output in ('', 'test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 10 filtered out;'):
            with self.assertRaises(ValueError):
                GATE.validate_result(output, 3, 10, 0, 1)

    def test_namespace_child_summary_cannot_replace_parent_completion(self):
        child = 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;\n'
        parent = 'test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out;\n'
        self.assertEqual(GATE.validate_result(child+parent, 2, 5, 0, 0)['passed'], 2)
        with self.assertRaises(ValueError):
            GATE.validate_result(child, 2, 5, 0, 0)

    def test_full_successful_receipts_form_exact_union(self):
        self.verify(self.receipts())

    def test_missing_duplicate_unknown_failed_and_wrong_head_receipts_fail(self):
        for change in ('missing', 'duplicate', 'unknown', 'failed', 'head', 'lock'):
            receipts = self.receipts()
            if change == 'missing': receipts.pop()
            elif change == 'duplicate': receipts.append(copy.deepcopy(receipts[0]))
            elif change == 'unknown': receipts[0]['shard'] = 'tests-88'
            elif change == 'failed': receipts[0]['status'] = 'incomplete'
            elif change == 'head': receipts[0]['head'] = 'old-head'
            else: receipts[0]['lockfile_sha256'] = 'different'
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.verify(receipts)

    def test_missing_repeated_and_cross_shard_tests_fail(self):
        for change in ('missing', 'duplicate', 'wrong-target', 'failed', 'inventory', 'weights', 'counts', 'command'):
            receipts = self.receipts()
            result = receipts[0]['results'][0]
            if change == 'missing': result['assigned'].pop()
            elif change == 'duplicate': result['assigned'].append(result['assigned'][0])
            elif change == 'wrong-target': result['target'] = 'different-target'
            elif change == 'failed': result['status'] = 'failure'
            elif change == 'inventory': receipts[0]['inventory'][0]['tests'].pop()
            elif change == 'weights': receipts[0]['weights_sha256'] = 'other'
            elif change == 'counts': result['counts']['passed'] = 0
            else: receipts[0]['commands'][0]['returncode'] = 1
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.verify(receipts)

    def test_auxiliary_exact_command_inventory_cannot_shrink_or_repeat(self):
        for change in ('missing', 'duplicate', 'failed'):
            receipts = self.receipts()
            commands = receipts[-1]['commands']
            if change == 'missing': commands.pop()
            elif change == 'duplicate': commands.append(copy.deepcopy(commands[-1]))
            else: commands[-1]['returncode'] = 1
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.verify(receipts)

    def test_exact_ignored_rejects_zero_tests_and_requires_each_receipt(self):
        good = 'test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 99 filtered out;\n'
        self.assertEqual(GATE.exact_ignored_result(good+good)['passed'], 1)
        for output in ('', good.replace('1 passed', '0 passed'), good.replace('1 passed', '2 passed'),
                       good.replace('0 ignored', '1 ignored'), good.replace('ok.', 'FAILED.')):
            with self.subTest(output=output), self.assertRaises(ValueError):
                GATE.exact_ignored_result(output)
        for change in ('missing', 'name', 'zero'):
            receipts = self.receipts()
            results = receipts[-1]['results']
            if change == 'missing': results.pop()
            elif change == 'name': results[0]['name'] = 'different'
            else: results[0]['counts']['passed'] = 0
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.verify(receipts)

    def test_all_original_io_and_ignored_commands_are_retained_once(self):
        original = (ROOT / '.github/scripts/check.sh').read_text().split('  workspace)\n', 1)[1].split('    ;;', 1)[0]
        import re
        scripts = re.findall(r'^    python3 (\.github/scripts/check-.*\.py)$', original, re.M)
        commands = [command for shard in GATE.SHARDS if not shard.startswith('tests-') for command in GATE.auxiliary_commands(shard)]
        self.assertEqual(Counter(command[1] for command in commands if command[0] == 'python3'), Counter(scripts))
        for name in GATE.IGNORED:
            self.assertIn(name, original)
            self.assertEqual(sum(name in command for command in commands), 1)
        self.assertEqual(sum('--doc' in command for command in commands), 1)
        self.assertEqual(sum(command[1] == 'check' for command in commands if command[0] == 'cargo'), 2)

    def test_policy_expands_only_workspace_and_never_filters_the_matrix(self):
        matrix = POLICY.execution_matrix()
        self.assertEqual(tuple(POLICY.WORKSPACE_SHARDS), GATE.SHARDS)
        self.assertEqual([r['shard'] for r in matrix if r['suite'] == 'workspace'], list(GATE.SHARDS))
        self.assertEqual(Counter(r['suite'] for r in matrix if r['suite'] != 'workspace'), Counter(s for s in POLICY.FULL_SUITES if s != 'workspace'))
        for rows in (matrix[:-1], matrix+[matrix[0]], []):
            receipt = POLICY.receipt(); receipt['matrix'] = rows
            with self.assertRaises(ValueError): POLICY.check_receipt(receipt)

    def test_cargo_inventory_uses_all_default_member_targets_and_actual_artifacts(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary); binary=root/'built'; binary.write_text('not executed')
            (root/'Cargo.toml').write_text('[package]\nname="example"\nversion="0.0.0"\n')
            target={'kind':['lib'], 'name':'example', 'test':True}
            package={'id':'package-id', 'manifest_path':str(root/'Cargo.toml'), 'targets':[target]}
            metadata={'packages':[package], 'workspace_default_members':['package-id']}
            artifact={'reason':'compiler-artifact', 'package_id':'package-id', 'features':[], 'profile':{'test':True}, 'executable':str(binary), 'target':target}
            finish={'reason':'build-finished', 'success':True}
            output='\n'.join(map(json.dumps,[artifact,finish]))
            self.assertEqual(GATE.artifacts(output,metadata,root)[0]['id'], 'Cargo.toml:lib:example')
            for rows in ([finish], [artifact,artifact,finish], [artifact,dict(finish,success=False)]):
                with self.assertRaises(ValueError): GATE.artifacts('\n'.join(map(json.dumps,rows)),metadata,root)
            (root/'Cargo.toml').write_text('[lib]\nharness=false\n')
            with self.assertRaisesRegex(ValueError,'custom test harness'):
                GATE.artifacts(output,metadata,root)
            (root/'Cargo.toml').write_text('[lib]\n')
            metadata['packages'][0]['targets'].append(dict(target,name='missing'))
            with self.assertRaisesRegex(ValueError,'missing'): GATE.artifacts(output,metadata,root)


if __name__ == '__main__':
    unittest.main()
