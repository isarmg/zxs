"""Continuous default-budget engineering control; never replaces frozen e2f gates."""
import json
import os
from pathlib import Path
import re
import sys
import time

import candidate_220 as original

screen = original.screen
CONTROL_TREE = '378f3b063c8aa2099da3d582eaa511937da138e9'
CONTROL_DELTA = 'src/engine/src/engine/workspace_budget.rs'


def configure(product_sha, product_tree, control_sha, control_tree):
    original.configure(product_sha, product_tree)
    assert re.fullmatch('[0-9a-f]{40}', control_sha)
    assert control_sha not in (original.BASELINE[0], product_sha)
    assert control_tree == CONTROL_TREE, 'control must have the reviewed sole ID-width delta'
    screen.PINS['baseline'] = (control_sha, control_tree)
    return {key: list(value) for key, value in screen.PINS.items()}


def provenance(pins):
    return dict(kind='e2f-plus-only-common-ID-width-repair', original=list(original.BASELINE),
                actual=pins['baseline'], only_delta=CONTROL_DELTA,
                scope='engineering attribution only; not the original frozen baseline gate')


def prepare(workspace, evidence, pins):
    control = workspace / 'baseline'
    assert screen.git(control, 'rev-parse', 'HEAD^') == original.BASELINE[0]
    assert screen.git(control, 'diff', '--name-only', original.BASELINE[0], 'HEAD') == CONTROL_DELTA
    screen.prepare(workspace, evidence)
    path = evidence / 'controller.json'
    controller = json.loads(path.read_text())
    controller['engineering_control'] = provenance(pins)
    screen.write(path, controller)


def checked_manifest(bundle, pins):
    manifest = original.checked_manifest(bundle, pins)
    assert manifest['controller']['engineering_control'] == provenance(pins)
    return manifest


def select(bundle, output, pins):
    checked_manifest(bundle, pins)
    now = int(time.time())
    screen.write(output, dict(profile='continuous-engineering-220', source_pins=pins,
        controller_sha=os.environ['GITHUB_SHA'], normal_source_sha=os.environ['RPC_SOURCE_SHA'],
        manifest_sha256=os.environ['RPC_MANIFEST_SHA256'], created_unix=now, deadline_unix=now+45*60,
        phase_seconds=1200, pair_seconds=45*60, cpus=2, address_space_bytes=8*screen.GIB,
        minimum_free_bytes=8*screen.GIB, principal='bench', initial_sequence=220, final_sequence=360,
        principal_metadata_limit=64*1024*1024, global_metadata_limit=128*1024*1024,
        engineering_control=provenance(pins), original_baseline_gate='separate original-220 run required',
        old_original_220=dict(run_id=41174023827, status='FAIL retained'),
        old_continuous=dict(run_id=41168852801, status='FAIL_DEFAULT_BUDGET retained; old 64-byte reservation')))


def checked_plan(path, pins):
    assert screen.sha(path) == os.environ['RPC_PLAN_SHA256']
    plan = json.loads(path.read_text())
    assert plan['profile'] == 'continuous-engineering-220' and plan['source_pins'] == pins
    assert plan['engineering_control'] == provenance(pins)
    assert plan['controller_sha'] == os.environ['GITHUB_SHA']
    assert plan['normal_source_sha'] == os.environ['RPC_SOURCE_SHA']
    assert plan['manifest_sha256'] == os.environ['RPC_MANIFEST_SHA256']
    assert plan['deadline_unix'] - plan['created_unix'] == plan['pair_seconds'] == 45*60
    assert plan['phase_seconds'] == 1200 and plan['cpus'] == 2
    assert plan['address_space_bytes'] == plan['minimum_free_bytes'] == 8*screen.GIB
    assert (plan['principal'], plan['initial_sequence'], plan['final_sequence']) == ('bench', 220, 360)
    assert plan['principal_metadata_limit'] == 64*1024*1024 and plan['global_metadata_limit'] == 128*1024*1024
    return plan


def run(bundle, output, pair, path, pins):
    assert pair in (0, 1)
    plan = checked_plan(path, pins)
    assert plan['created_unix'] <= time.time() < plan['deadline_unix']
    checked_manifest(bundle, pins)
    screen.run(bundle, output, 220, pair, continuous_only=True, execution_deadline=plan['deadline_unix'])


def count(value):
    assert type(value) is int or (type(value) is str and re.fullmatch('0|[1-9][0-9]*', value))
    value = int(value)
    assert value >= 0
    return value


def retained_oracle(after):
    for name, expected in dict(retained_seed_receipts=220, retained_seed_bodies=718,
                               retained_rpc_receipts=20, retained_rpc_bodies=20,
                               retained_total_operations=360, retained_total_events=360,
                               retained_workspaces=359).items():
        assert after[name] == expected, 'missing retained history: ' + name
    assert after['principal'] == 'bench'
    capacity = after['capacity']
    for scope, limit in [('principal', 64*1024*1024), ('global', 128*1024*1024)]:
        actual = capacity[scope]
        assert count(actual['metadata_bytes']['limit']) == limit
        assert 0 < count(actual['metadata_bytes']['used']) <= limit
        assert count(actual['records']['used']) + count(actual['cold_records']) == 359
    return dict(operations=360, workspaces=359, principal='bench', capacity=capacity)


def envelope(data, pair, plan):
    assert data['profile'] == 'continuous-220'
    assert data['controller_sha'] == plan['controller_sha'] and data['normal_source_sha'] == plan['normal_source_sha']
    assert data['manifest_sha256'] == plan['manifest_sha256'] and not data['reused_build']
    assert data['commits'] == 220 and data['pair'] == pair
    order = ['baseline', 'candidate'] if pair == 0 else ['candidate', 'baseline']
    assert data['order'] == order
    assert [arm['label'] for arm in data['runs']] == order[:len(data['runs'])]
    assert 0 < len(data['runs']) <= 2 and len(data['host']['child_cpus']) == 2


def candidate_sample(data, pair, plan):
    envelope(data, pair, plan)
    arms = [arm for arm in data['runs'] if arm['label'] == 'candidate']
    if not arms:
        return dict(pair=pair, status='NOT EXECUTED')
    screen.validate_arm(arms[0], 220)
    return dict(pair=pair, status='PASS', retained=retained_oracle(arms[0]['phases']['tails-verify']))


def compare(data, pair, plan):
    envelope(data, pair, plan)
    result = screen.compare_pair(data, 220, pair, plan['manifest_sha256'])
    result['retained'] = {arm['label']: retained_oracle(arm['phases']['tails-verify']) for arm in data['runs']}
    assert len(result['checks']) == 11
    return result


def summary(root, path, output, pins):
    plan = checked_plan(path, pins)
    report = dict(status='incomplete', source_pins=pins, engineering_control=provenance(pins),
        manifest_sha256=plan['manifest_sha256'], candidate_continuous_capacity='NOT EVALUATED',
        delivery_policy=screen.DELIVERY_POLICY, delivery_gate='NOT EVALUATED',
        engineering_eleven_gate='NOT EVALUATED', engineering_tail_latency_gate='NOT EVALUATED',
        original_baseline_gate='NOT EVALUATED HERE; use separate frozen-e2f original-220 result',
        old_original_220=plan['old_original_220'], old_continuous=plan['old_continuous'], old_2af_gate='FAIL retained', pairs=[])
    report.update(candidate_samples=[], candidate_samples_passed=0, candidate_samples_required=2, pair_errors=[])
    for pair in (0, 1):
        file = root / f'rpc-220-continuous-{pair}' / 'pair.json'
        try:
            data = json.loads(file.read_text())
        except (OSError, ValueError) as error:
            report['pair_errors'].append(dict(pair=pair, error=str(error)))
            continue
        try:
            sample = candidate_sample(data, pair, plan)
        except (AssertionError, KeyError, TypeError, ValueError) as error:
            sample = dict(pair=pair, status='NOT VERIFIED', error=str(error))
        report['candidate_samples'].append(sample)
        try:
            report['pairs'].append(compare(data, pair, plan))
        except (AssertionError, KeyError, TypeError, ValueError) as error:
            report['pair_errors'].append(dict(pair=pair, error=str(error)))
    passed = sum(sample['status'] == 'PASS' for sample in report['candidate_samples'])
    report['candidate_samples_passed'] = passed
    if passed:
        report['candidate_continuous_capacity'] = 'PASS' if passed == 2 else 'PARTIAL: 1/2 verified'
    if len(report['pairs']) == 2:
        all_pass = all(check['passed'] for row in report['pairs'] for check in row['checks'].values())
        tails_pass = all(row['checks'][name+'_p95']['passed'] for row in report['pairs'] for name in ('flush', 'close', 'write_through'))
        report.update(status='completed', engineering_eleven_gate='PASS' if all_pass else 'FAIL',
                      engineering_tail_latency_gate='PASS' if tails_pass else 'FAIL',
                      delivery_gate=screen.delivery_gate(report['pairs']))
    screen.write(output, report)
    print('CONTINUOUS_ENGINEERING_220', json.dumps(report))
    if report['delivery_gate'] != 'PASS' or report['candidate_continuous_capacity'] != 'PASS':
        raise SystemExit(1)


if __name__ == '__main__':
    action, *args = sys.argv[1:]
    pins = configure(*args[-4:])
    args = args[:-4]
    if action == 'prepare': prepare(*map(Path, args), pins)
    elif action == 'seal': screen.seal(*map(Path, args))
    elif action == 'select': select(*map(Path, args), pins)
    elif action == 'run': run(Path(args[0]), Path(args[1]), int(args[2]), Path(args[3]), pins)
    elif action == 'summary': summary(*map(Path, args), pins)
    else: raise ValueError('unsupported action')
