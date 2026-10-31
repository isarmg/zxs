"""New sealed product, original 220 contract; never relabel old frozen evidence."""
import json
import os
from pathlib import Path
import re
import sys
import time

import screen

BASELINE = ('e2f33d9882cf7263412d9370b6a703e0b216357d', 'bee9d4686088a4af6c659a983c6e711f5f62d272')


def configure(product_sha, product_tree):
    assert re.fullmatch('[0-9a-f]{40}', product_sha), 'exact product SHA required'
    assert re.fullmatch('[0-9a-f]{40}', product_tree), 'exact product tree required'
    assert product_sha != BASELINE[0], 'candidate cannot be the baseline'
    assert not os.environ.get('RPC_REUSE_BUILD'), 'old sealed candidate cannot be reused'
    # This standalone controller sets its immutable product selection once;
    # the original screen entry point retains its historical fixed PINS.
    screen.PINS = dict(baseline=BASELINE, candidate=(product_sha, product_tree))
    return {key: list(value) for key, value in screen.PINS.items()}


def checked_manifest(bundle, pins):
    path = bundle / 'manifest.json'
    assert screen.sha(path) == os.environ['RPC_MANIFEST_SHA256'], 'manifest hash differs'
    manifest = json.loads(path.read_text())
    controller = manifest['controller']
    assert controller['source_pins'] == pins, 'sealed product selection differs'
    assert controller['sha'] == os.environ['GITHUB_SHA'], 'carrier differs'
    assert controller['normal_source_sha'] == os.environ['RPC_SOURCE_SHA'] == screen.git(screen.ROOT, 'rev-parse', 'HEAD')
    assert controller['normal_source_tree'] == screen.git(screen.ROOT, 'rev-parse', 'HEAD^{tree}')
    assert set(manifest['source']) == set(pins)
    assert set(manifest['binaries']) == {f'{label}-{kind}' for label in pins for kind in ('seed', 'server')}
    for label, (commit, tree) in pins.items():
        identity = manifest['source'][label]
        assert (identity['product_sha'], identity['base_tree']) == (commit, tree), 'sealed source/tree differs'
        assert identity['production_source_changes'] is False
        for kind in ('seed', 'server'):
            name = f'{label}-{kind}'
            assert screen.sha(bundle / name) == manifest['binaries'][name], 'binary hash differs'
    return manifest


def select(bundle, output, pins):
    checked_manifest(bundle, pins)
    now = int(time.time())
    screen.write(output, dict(profile='candidate-original-220', source_pins=pins,
        controller_sha=os.environ['GITHUB_SHA'], normal_source_sha=os.environ['RPC_SOURCE_SHA'],
        manifest_sha256=os.environ['RPC_MANIFEST_SHA256'], created_unix=now, deadline_unix=now + 45*60,
        phase_seconds=1200, pair_seconds=45*60, address_space_bytes=8*screen.GIB,
        minimum_free_bytes=8*screen.GIB, cpus=2,
        old_original_220=dict(run_id=41174023827, status='FAIL retained'),
        old_22=dict(run_id=41168852801, scope='historical frozen d3 only; not new-product coverage'),
        old_continuous=dict(run_id=41168852801, status='FAIL_DEFAULT_BUDGET retained; old 64-byte reservation',
                            budget_proof=screen.budget_proof()),
        actual_continuous_run=None, continuous_extension_support='NOT_EVALUATED_BY_ORIGINAL_220'))


def checked_plan(path, pins):
    assert screen.sha(path) == os.environ['RPC_PLAN_SHA256'], 'plan hash differs'
    plan = json.loads(path.read_text())
    assert plan['profile'] == 'candidate-original-220' and plan['source_pins'] == pins
    assert plan['controller_sha'] == os.environ['GITHUB_SHA']
    assert plan['normal_source_sha'] == os.environ['RPC_SOURCE_SHA']
    assert plan['manifest_sha256'] == os.environ['RPC_MANIFEST_SHA256']
    assert plan['deadline_unix'] - plan['created_unix'] == plan['pair_seconds'] == 45*60
    assert plan['phase_seconds'] == 1200 and plan['cpus'] == 2
    assert plan['address_space_bytes'] == plan['minimum_free_bytes'] == 8*screen.GIB
    assert plan['continuous_extension_support'] == 'NOT_EVALUATED_BY_ORIGINAL_220'
    assert plan['actual_continuous_run'] is None
    assert plan['old_continuous'] == dict(run_id=41168852801,
        status='FAIL_DEFAULT_BUDGET retained; old 64-byte reservation', budget_proof=screen.budget_proof())
    return plan


def run(bundle, output, pair, path, pins):
    assert pair in (0, 1)
    plan = checked_plan(path, pins)
    assert plan['created_unix'] <= time.time() < plan['deadline_unix']
    checked_manifest(bundle, pins)
    screen.run(bundle, output, 220, pair, original_only=True, execution_deadline=plan['deadline_unix'])


def compare(data, pair, plan):
    assert data['profile'] == 'original-220' and data['status'] == 'completed'
    assert data['commits'] == 220 and data['pair'] == pair
    assert data['manifest_sha256'] == plan['manifest_sha256']
    assert data['controller_sha'] == plan['controller_sha']
    assert data['normal_source_sha'] == plan['normal_source_sha']
    assert not data['reused_build']
    order = ['baseline', 'candidate'] if pair == 0 else ['candidate', 'baseline']
    assert data['order'] == order and [arm['label'] for arm in data['runs']] == order
    assert len(data['host']['child_cpus']) == 2
    for arm in data['runs']:
        assert arm['status'] == 'completed'
        assert set(arm['phases']) == {'build', 'edit', 'verify', 'requests', 'rpc-verify'}
        screen.validate_original_phases(arm['phases'], 220)
    return dict(pair=pair, checks=screen.original_checks({arm['label']: arm for arm in data['runs']}, 220))


def summary(root, path, output, pins):
    plan = checked_plan(path, pins)
    report = dict(status='incomplete', source_pins=pins, manifest_sha256=plan['manifest_sha256'],
        original_220_gate='NOT EVALUATED', continuous_extension_support='NOT_EVALUATED_BY_ORIGINAL_220',
        full_extension_acceptance='NOT_EVALUATED_BY_ORIGINAL_220',
        actual_continuous_run=None, old_continuous=plan['old_continuous'],
        exit_scope='validated original pairs and remaining blocking checks only; never full continuous acceptance',
        delivery_policy=screen.DELIVERY_POLICY, delivery_gate='NOT EVALUATED',
        old_original_220=plan['old_original_220'],
        old_22=plan['old_22'], old_2af_gate='FAIL unchanged', pairs=[])
    try:
        for pair in (0, 1):
            data = json.loads((root / f'rpc-220-original-{pair}' / 'pair.json').read_text())
            report['pairs'].append(compare(data, pair, plan))
        passed = all(check['passed'] for row in report['pairs'] for check in row['checks'].values())
        report.update(status='completed', original_220_gate='PASS' if passed else 'FAIL',
                      delivery_gate=screen.delivery_gate(report['pairs']))
    finally:
        screen.write(output, report)
        print('CANDIDATE_ORIGINAL_220', json.dumps(report))
    # Exit success certifies validated pairs and current blocking checks only.
    # This five-stage controller never evaluates the separate continuous run.
    if report['delivery_gate'] != 'PASS':
        raise SystemExit(1)


if __name__ == '__main__':
    action, *args = sys.argv[1:]
    product_sha, product_tree = args[-2:]
    pins = configure(product_sha, product_tree)
    args = args[:-2]
    if action == 'prepare': screen.prepare(*map(Path, args))
    elif action == 'seal': screen.seal(*map(Path, args))
    elif action == 'select': select(*map(Path, args), pins)
    elif action == 'run': run(Path(args[0]), Path(args[1]), int(args[2]), Path(args[3]), pins)
    elif action == 'summary': summary(*map(Path, args), pins)
    else: raise ValueError('unsupported action')
