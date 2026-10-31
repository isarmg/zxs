"""Recover only the missing candidate capacity sample from an exact old seal."""
import json
import os
from pathlib import Path
import re
import shutil
import sys
import tempfile
import time
import traceback

import continuous_220 as continuous

screen = continuous.screen
BUILD_RUN = 41180163138
BINARY_ARTIFACT = 13034728252
PRODUCT = ['52b5901ac716f67432fe01e9515c1d7158a480e9', 'ee672528c69b4b00428f04d015451037aed8511b']
CONTROL = ['51b6af2c2febf18b65f367ce03ec8f82c0e8ee37', continuous.CONTROL_TREE]
OLD_CARRIER = '8c8531b7b32be6cc7bb56d202b71fcaae011dad5'
MANIFEST_SHA = '78027232da8013e4b0a46e88be772900afd4c314df9330e61bd55ffd31efd04b'
PRIOR_PAIR_SHA = 'b8f1fef25709270122a6cbb870371782f59f5e59ccd8e94210ba6be9940d4dba'
PHASES = ('build', 'edit', 'verify', 'requests', 'rpc-verify', 'tails', 'tails-verify')


def observer():
    source = os.environ['RPC_SOURCE_SHA']
    carrier = os.environ['GITHUB_SHA']
    assert re.fullmatch('[0-9a-f]{40}', source) and re.fullmatch('[0-9a-f]{40}', carrier)
    assert screen.git(screen.ROOT, 'rev-parse', 'HEAD') == source
    assert not screen.git(screen.ROOT, 'status', '--porcelain')
    return dict(source_sha=source, source_tree=screen.git(screen.ROOT, 'rev-parse', 'HEAD^{tree}'),
                carrier_sha=carrier, run_id=os.environ['GITHUB_RUN_ID'])


def checked_bundle(bundle):
    assert not os.environ.get('RPC_REUSE_BUILD')
    assert screen.sha(bundle/'manifest.json') == MANIFEST_SHA
    manifest = json.loads((bundle/'manifest.json').read_text())
    controller = manifest['controller']
    assert controller['run_id'] == str(BUILD_RUN) and controller['sha'] == OLD_CARRIER
    assert [controller['normal_source_sha'], controller['normal_source_tree']] == PRODUCT
    pins = dict(baseline=CONTROL, candidate=PRODUCT)
    assert controller['source_pins'] == pins and set(manifest['source']) == set(pins)
    assert controller['engineering_control'] == continuous.provenance(pins)
    for label, (sha, tree) in pins.items():
        identity = manifest['source'][label]
        assert [identity['product_sha'], identity['base_tree']] == [sha, tree]
        assert identity['production_source_changes'] is False
    assert set(manifest['binaries']) == {label+'-'+kind for label in pins for kind in ('seed', 'server')}
    for name, digest in manifest['binaries'].items():
        assert screen.sha(bundle/name) == digest, 'sealed binary changed'
    return manifest


def prior_sample(path):
    assert screen.sha(path) == PRIOR_PAIR_SHA, 'prior BA evidence changed'
    data = json.loads(path.read_text())
    sample = continuous.candidate_sample(data, 1, dict(controller_sha=OLD_CARRIER,
        normal_source_sha=PRODUCT[0], manifest_sha256=MANIFEST_SHA))
    assert sample['status'] == 'PASS'
    return sample


def select(bundle, prior, output):
    checked_bundle(bundle)
    prior_sample(prior)
    now = int(time.time())
    screen.write(output, dict(profile='candidate-capacity-recovery-220', observer=observer(),
        product=PRODUCT, build_run=BUILD_RUN, binary_artifact=BINARY_ARTIFACT,
        manifest_sha256=MANIFEST_SHA, prior_pair_sha256=PRIOR_PAIR_SHA,
        created_unix=now, deadline_unix=now+2700, phase_seconds=1200, sample_seconds=2700,
        cpus=2, address_space_bytes=8*screen.GIB, minimum_free_bytes=8*screen.GIB,
        principal='bench', initial_sequence=220, final_sequence=360,
        principal_metadata_limit=64*1024*1024, global_metadata_limit=128*1024*1024))


def checked_plan(path):
    assert screen.sha(path) == os.environ['RPC_PLAN_SHA256']
    plan = json.loads(path.read_text())
    assert plan['profile'] == 'candidate-capacity-recovery-220' and plan['observer'] == observer()
    assert plan['product'] == PRODUCT and plan['build_run'] == BUILD_RUN
    assert plan['binary_artifact'] == BINARY_ARTIFACT
    assert plan['manifest_sha256'] == MANIFEST_SHA and plan['prior_pair_sha256'] == PRIOR_PAIR_SHA
    assert plan['deadline_unix']-plan['created_unix'] == plan['sample_seconds'] == 2700
    assert plan['phase_seconds'] == 1200 and plan['cpus'] == 2
    assert plan['address_space_bytes'] == plan['minimum_free_bytes'] == 8*screen.GIB
    assert (plan['principal'],plan['initial_sequence'],plan['final_sequence']) == ('bench',220,360)
    assert plan['principal_metadata_limit'] == 64*1024*1024 and plan['global_metadata_limit'] == 128*1024*1024
    return plan


def run(bundle, prior, path, output):
    assert os.environ.get('GITHUB_ACTIONS') == 'true'
    plan = checked_plan(path)
    assert plan['created_unix'] <= time.time() < plan['deadline_unix']
    checked_bundle(bundle)
    prior = prior_sample(prior)
    output.mkdir(parents=True, exist_ok=False)
    arm = dict(label='candidate', status='incomplete', phases={})
    report = dict(status='incomplete', profile=plan['profile'], observer=plan['observer'],
        product=PRODUCT, build_run=BUILD_RUN, binary_artifact=BINARY_ARTIFACT,
        original_carrier=OLD_CARRIER, manifest_sha256=MANIFEST_SHA,
        prior_pair_sha256=PRIOR_PAIR_SHA, prior_candidate_sample=prior,
        candidate_samples_passed=1, candidate_samples_required=2,
        candidate_continuous_capacity='PARTIAL: 1/2 verified',
        original_abba='INCOMPLETE; original deadlines and artifacts unchanged',
        performance_scope='observations only; no paired comparison performed', arm=arm)
    try:
        report['host'] = screen.host_observation()
        assert len(report['host']['child_cpus']) == 2
        for kind in ('seed','server'):
            (bundle/('candidate-'+kind)).chmod(0o755)
        with tempfile.TemporaryDirectory(prefix='capacity-', dir=os.environ['RUNNER_TEMP']) as tmp:
            root = Path(tmp)
            env = dict(os.environ, ZXS_RPC_COMMITS='220', ZXS_RPC_ROOT=tmp,
                       RAYON_NUM_THREADS='2', TOKIO_WORKER_THREADS='2')
            for phase in PHASES:
                assert time.time() < plan['deadline_unix'], 'recovery window expired'
                assert shutil.disk_usage(output).free >= 8*screen.GIB, 'minimum free space unavailable'
                command = ([str(bundle/'candidate-server'), '--ignored', '--exact',
                    screen.FILTER if phase == 'requests' else screen.TAIL_FILTER, '--nocapture', '--test-threads=1']
                    if phase in ('requests','tails') else [str(bundle/'candidate-seed'), phase, '500', tmp])
                arm['phases'][phase] = screen.phase(command, env, output, phase, plan['deadline_unix'])
                if phase in ('verify','rpc-verify','tails-verify'):
                    arm.setdefault('inventory',{})[phase] = dict(metadata=screen.inventory(root/'d1/metadata'),
                        immutable_objects=screen.inventory(root/'d1/metadata/objects'),state=screen.inventory(root/'state'))
                for name in ({'build':['operations'],'edit':['operations'],
                              'requests':['rpc-receipts.json'],'tails':['tails-receipts.json']}.get(phase, [])):
                    shutil.copyfile(root/name,output/('seed-operation-ids.txt' if name=='operations' else name))
                screen.write(output/'summary.json',report)
        assert time.time() < plan['deadline_unix'], 'recovery window expired'
        arm['status'] = 'completed'
        screen.validate_arm(arm,220)
        report['retained'] = continuous.retained_oracle(arm['phases']['tails-verify'])
        report.update(status='completed',candidate_samples_passed=2,candidate_continuous_capacity='PASS')
    except BaseException:
        report['error'] = traceback.format_exc()
        raise
    finally:
        screen.write(output/'summary.json',report)
        print('CANDIDATE_CAPACITY_RECOVERY',json.dumps(report))


if __name__ == '__main__':
    action, *args = sys.argv[1:]
    if action == 'select': select(*map(Path,args))
    elif action == 'run': run(*map(Path,args))
    else: raise ValueError('unsupported action')
