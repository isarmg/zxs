"""Single bounded local-RPC experiment. Runtime is GitHub Actions only."""
import hashlib
import json
import math
import os
from pathlib import Path
import resource
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import traceback

PINS = {
    'baseline': ('e2f33d9882cf7263412d9370b6a703e0b216357d', 'bee9d4686088a4af6c659a983c6e711f5f62d272'),
    'candidate': ('2af128b5d1eaee2bbb7b5223f8574eeb1207f7c3', 'bbedf3f2dc7af196a276bf6fd968f79fa821db4b'),
}
FILTER = 'smb_rpc::tests::request_screen::bounded_request_screen'
GIB = 1024 ** 3
HERE = Path(__file__).resolve().parent
WIRING = b'\n[[bin]]\nname="local_rpc_seed"\npath="../../test/engine/bench/local_rpc_seed.rs"\ntest=false\nbench=false\n'
TEST_WIRING = b'\n#[path = "request_screen.rs"]\nmod request_screen;\n'
TRACKED = ['src/engine/Cargo.toml', 'test/server/unit/smb_rpc/tests.rs']
ADDED = ['test/engine/bench/local_rpc_seed.rs', 'test/server/unit/smb_rpc/request_screen.rs']

def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()

def write(path, value):
    Path(path).write_text(json.dumps(value, indent=2) + '\n')

def git(root, *args, **kwargs):
    return subprocess.check_output(['git', '-C', str(root), *args], **kwargs).decode().strip()

def effective_tree(root):
    with tempfile.TemporaryDirectory() as tmp:
        env = dict(os.environ, GIT_INDEX_FILE=str(Path(tmp) / 'index'))
        git(root, 'read-tree', 'HEAD', env=env)
        git(root, 'add', '--', *TRACKED, *ADDED, env=env)
        return git(root, 'write-tree', env=env)

def identity(root):
    assert set(git(root, 'diff', '--name-only').splitlines()) == set(TRACKED)
    assert set(git(root, 'ls-files', '--others', '--exclude-standard').splitlines()) == set(ADDED)
    changes = {}
    for path in TRACKED + ADDED:
        before = subprocess.check_output(['git', '-C', str(root), 'show', 'HEAD:' + path]) if path in TRACKED else None
        changes[path] = dict(before_sha256=hashlib.sha256(before).hexdigest() if before is not None else None,
                             after_sha256=sha(root / path))
    return dict(product_sha=git(root, 'rev-parse', 'HEAD'), base_tree=git(root, 'rev-parse', 'HEAD^{tree}'),
                effective_test_build_tree=effective_tree(root), overlay=changes,
                production_source_changes=False)

def prepare(workspace, evidence):
    evidence.mkdir(parents=True, exist_ok=False)
    identities = {}
    for label, (commit, tree) in PINS.items():
        root = workspace / label
        assert git(root, 'rev-parse', 'HEAD') == commit
        assert git(root, 'rev-parse', 'HEAD^{tree}') == tree
        assert not git(root, 'status', '--porcelain')
        for path, suffix in zip(TRACKED, (WIRING, TEST_WIRING)):
            target = root / path
            target.write_bytes(target.read_bytes() + suffix)
        (root / ADDED[0]).parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(HERE / (label + '_seed.rs'), root / ADDED[0])
        shutil.copyfile(HERE / 'request_screen.rs', root / ADDED[1])
        identities[label] = identity(root)
    write(evidence / 'source-before.json', identities)
    write(evidence / 'controller.json', dict(sha=os.environ['GITHUB_SHA'], run_id=os.environ['GITHUB_RUN_ID'],
          files={p.name:sha(p) for p in HERE.iterdir() if p.is_file()},
          deadline_unix=int(time.time()) + 150 * 60,
          criteria=dict(read_list_p95_delta_ms=2, read_list_p99_delta_ms=5, save_batch_delta_ms=1000,
                        save_p95_delta_ms=50, open_vmhwm_reduction_220=0.20, startup_ratio_220=1.10),
          old_parity_gate='FAILED unchanged; independent prospective engineering screen',
          exclusions=['not user SLO','not capacity certification','not network SMB','not physical12disk'],
          protocol='500 files;22/220 seed commits;1 file branch;1000 read/list cycles;20 committed closes;AB/BA'))

def seal(workspace, evidence, bundle):
    before = json.loads((evidence / 'source-before.json').read_text())
    after = {label:identity(workspace / label) for label in PINS}
    assert before == after, 'build changed authenticated input'
    write(evidence / 'source-after.json', after)
    bundle.mkdir(parents=True, exist_ok=False)
    binaries = {}
    features = {}
    for label in PINS:
        rows = [json.loads(line) for line in (evidence / f'{label}-server.jsonl').read_text().splitlines() if line.startswith('{')]
        features[label] = [dict(target=row['target']['name'], features=row['features'], profile=row['profile'])
            for row in rows if row.get('reason') == 'compiler-artifact'
            and row['target']['name'] in ('storage_engine','storage_server')]
        engine_features = [row['features'] for row in features[label] if row['target'] == 'storage_engine']
        assert engine_features == ([['fault-injection']] if label == 'baseline' else [['fault-injection','test-support']]), engine_features
        matches = [row['executable'] for row in rows if row.get('reason') == 'compiler-artifact'
                   and row.get('executable') and row['target']['name'] == 'storage_server'
                   and row['profile']['test']]
        assert len(matches) == 1, matches
        seed = Path(os.environ['RUNNER_TEMP']) / f'rpc-target-{label}/x86_64-unknown-linux-gnu/release/local_rpc_seed'
        for kind, source in (('seed', seed), ('server', Path(matches[0]))):
            destination = bundle / f'{label}-{kind}'
            shutil.copyfile(source, destination)
            destination.chmod(0o755)
            binaries[destination.name] = sha(destination)
    manifest = dict(source=after, binaries=binaries, build_features=features,
                    build_scope='release server lib-test, existing dev-dependency engine features; no failpoints or test-support helpers activated',
                    controller=json.loads((evidence / 'controller.json').read_text()))
    write(bundle / 'manifest.json', manifest)
    shutil.copyfile(bundle / 'manifest.json', evidence / 'manifest.json')
    print('MANIFEST_SHA256=' + sha(bundle / 'manifest.json'))

def limit():
    resource.setrlimit(resource.RLIMIT_AS, (8*GIB, 8*GIB))

def inventory(root):
    total = dict(bytes=0, allocated_bytes=0, files=0)
    for directory, _, files in os.walk(root):
        for name in files:
            info = (Path(directory) / name).stat()
            total['bytes'] += info.st_size
            total['allocated_bytes'] += info.st_blocks * 512
            total['files'] += 1
    return total

def phase(command, env, directory, name, deadline):
    result = dict(name=name, command=command, status='running', started_ns=time.time_ns(),
                  timeout_seconds=1200, address_space_limit_bytes=8*GIB, minimum_free_bytes=8*GIB)
    write(directory / (name + '.json'), result)
    with (directory / (name + '.stdout')).open('w') as stdout, (directory / (name + '.stderr')).open('w') as stderr:
        child = subprocess.Popen(command, stdout=stdout, stderr=stderr, env=env,
                                 start_new_session=True, preexec_fn=limit)
        expiry = min(time.time() + 1200, deadline)
        while child.poll() is None:
            if time.time() >= expiry or shutil.disk_usage(directory).free < 8*GIB:
                result['status'] = 'resource_or_time_limit'
                os.killpg(child.pid, signal.SIGTERM)
                try:
                    child.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    os.killpg(child.pid, signal.SIGKILL)
                    child.wait()
                break
            time.sleep(0.25)
        result.update(returncode=child.returncode, finished_ns=time.time_ns())
        if result['status'] == 'running':
            result['status'] = 'completed' if child.returncode == 0 else 'failed'
    write(directory / (name + '.json'), result)
    assert result['status'] == 'completed', result
    raw = (directory / (name + '.stdout')).read_text()
    if name == 'requests':
        values = [json.loads(line.split('RPC_RESULT ', 1)[1]) for line in raw.splitlines() if 'RPC_RESULT ' in line]
        assert len(values) == 1 and '1 passed; 0 failed' in raw, raw[-3000:]
        return values[0]
    values = [json.loads(line) for line in raw.splitlines() if line.startswith('{')]
    assert len(values) == 1, raw[-3000:]
    return values[0]

def run(bundle, out, commits, pair):
    assert commits in (22,220) and pair in (0,1)
    manifest = json.loads((bundle / 'manifest.json').read_text())
    assert sha(bundle / 'manifest.json') == os.environ['RPC_MANIFEST_SHA256']
    assert manifest['controller']['sha'] == os.environ['GITHUB_SHA']
    for name, digest in manifest['binaries'].items():
        assert sha(bundle / name) == digest
        (bundle / name).chmod(0o755)
    out.mkdir(parents=True, exist_ok=False)
    report = dict(commits=commits, pair=pair, order=['baseline','candidate'] if pair == 0 else ['candidate','baseline'],
                  status='incomplete', manifest_sha256=sha(bundle / 'manifest.json'), runs=[],
                  host=dict(uname=list(os.uname()), cpu=Path('/proc/cpuinfo').read_text(),
                            memory=Path('/proc/meminfo').read_text()),
                  checkpoint_selection='not observable via existing public API; warnings recorded',
                  caveat='pre-request full verification/inventory may warm OS caches; not cold device')
    try:
        for label in report['order']:
            arm = out / label
            arm.mkdir()
            record = dict(label=label, status='incomplete', phases={})
            report['runs'].append(record)
            with tempfile.TemporaryDirectory(prefix='rpc-', dir=os.environ['RUNNER_TEMP']) as tmp:
                root = Path(tmp)
                env = dict(os.environ, ZXS_RPC_COMMITS=str(commits), ZXS_RPC_ROOT=tmp)
                for name in ('build','edit','verify','requests','rpc-verify'):
                    command = [str(bundle / f'{label}-server'), '--ignored', '--exact', FILTER, '--nocapture', '--test-threads=1'] if name == 'requests' else [str(bundle / f'{label}-seed'), name, '500', tmp]
                    record['phases'][name] = phase(command, env, arm, name, manifest['controller']['deadline_unix'])
                    if name in ('verify','rpc-verify'):
                        record.setdefault('inventory', {})[name] = dict(metadata=inventory(root/'d1/metadata'),
                            immutable_objects=inventory(root/'d1/metadata/objects'), state=inventory(root/'state'))
                    write(out / 'pair.json', report)
                shutil.copyfile(root/'rpc-receipts.json', arm/'rpc-receipts.json')
                shutil.copyfile(root/'operations', arm/'seed-operation-ids.txt')
            record['status'] = 'completed'
        report['status'] = 'completed'
    except BaseException:
        report['error'] = traceback.format_exc()
        raise
    finally:
        write(out / 'pair.json', report)
        print('RPC_PAIR', json.dumps(report))

def quantile(values, p):
    return sorted(values)[math.ceil(len(values)*p)-1]

def summary(root, out):
    report = dict(status='incomplete', old_parity_gate='FAILED unchanged', prospective_screen='NOT EVALUATED', pairs=[])
    try:
        for commits in (22,220):
            for pair in (0,1):
                path = root / f'rpc-{commits}-{pair}' / 'pair.json'
                data = json.loads(path.read_text())
                assert data['status'] == 'completed' and data['commits'] == commits and data['pair'] == pair
                assert data['manifest_sha256'] == os.environ['RPC_MANIFEST_SHA256']
                arms = {arm['label']:arm for arm in data['runs']}
                assert set(arms) == set(PINS)
                for arm in arms.values():
                    assert arm['status'] == 'completed'
                    oracle = arm['phases']['verify']
                    for key, value in dict(verified_current_files=500,verified_operations=commits,
                                          verified_commit_namespaces=commits,verified_bodies=500+commits-2).items():
                        assert oracle[key] == value
                    after = arm['phases']['rpc-verify']
                    assert after['sequence'] == commits+20 and after['current_files'] == 500
                    assert after['saved_receipts'] == after['saved_commit_namespaces'] == after['saved_historical_bodies'] == 20
                    requests = arm['phases']['requests']
                    assert requests['verified_reads'] == 1001 and requests['verified_lists'] == 1000 and requests['verified_receipts'] == 20
                    assert all(len(requests[key]) == 1000 for key in ('read_ns','read_open_ns','list_ns'))
                    assert len(requests['save_ns']) == 20
                b,c = [arms[label]['phases']['requests'] for label in PINS]
                checks = {}
                for kind in ('read','list'):
                    for percentile, tolerance in ((.95,2e6),(.99,5e6)):
                        baseline,candidate = quantile(b[kind+'_ns'],percentile),quantile(c[kind+'_ns'],percentile)
                        checks[f'{kind}_p{int(percentile*100)}'] = dict(baseline_ns=baseline,candidate_ns=candidate,
                            delta_ns=candidate-baseline,limit_ns=tolerance,passed=candidate-baseline<=tolerance)
                for name, baseline, candidate, tolerance in (
                    ('save_batch',b['save_batch_ns'],c['save_batch_ns'],1e9),
                    ('save_p95',quantile(b['save_ns'],.95),quantile(c['save_ns'],.95),50e6)):
                    checks[name] = dict(baseline_ns=baseline,candidate_ns=candidate,delta_ns=candidate-baseline,
                                        limit_ns=tolerance,passed=candidate-baseline<=tolerance)
                if commits == 220:
                    checks['open_vmhwm'] = dict(baseline_kib=b['open_memory']['vmhwm_kib'],candidate_kib=c['open_memory']['vmhwm_kib'],
                        passed=c['open_memory']['vmhwm_kib'] <= .8*b['open_memory']['vmhwm_kib'])
                    checks['startup'] = dict(baseline_ns=b['open_ns'],candidate_ns=c['open_ns'],passed=c['open_ns']<=1.1*b['open_ns'])
                report['pairs'].append(dict(commits=commits,pair=pair,checks=checks,
                    metadata={label:arms[label]['inventory'] for label in PINS},
                    warnings={label:arms[label]['phases']['requests']['recovery_warnings'] for label in PINS}))
        report.update(status='completed',prospective_screen='PASS' if all(check['passed'] for pair in report['pairs'] for check in pair['checks'].values()) else 'FAIL')
    finally:
        write(out, report)
        print('RPC_SCREEN',json.dumps(report))

if __name__ == '__main__':
    action, *args = sys.argv[1:]
    if action == 'prepare': prepare(*map(Path,args))
    elif action == 'seal': seal(*map(Path,args))
    elif action == 'run': run(Path(args[0]),Path(args[1]),int(args[2]),int(args[3]))
    elif action == 'summary': summary(*map(Path,args))
    else: raise ValueError('unsupported action')
