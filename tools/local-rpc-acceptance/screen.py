"""Finite same-run AB/BA RPC acceptance; only GitHub Actions executes workloads."""
import os
import sys
if not __debug__ or os.environ.get("PYTHONOPTIMIZE"):
    raise SystemExit("optimized Python cannot execute acceptance assertions")
import hashlib
import json
import math
from pathlib import Path
import resource
import shutil
import signal
import subprocess
import tempfile
import time
import traceback

PINS = {
    'baseline': ('e2f33d9882cf7263412d9370b6a703e0b216357d', 'bee9d4686088a4af6c659a983c6e711f5f62d272'),
    'candidate': ('d3eefd289e53a2803703870fe4250d08cadff9b5', '52d5076189204952dbfaf4d3c2c76b93553d1b3e'),
}
FILTER = 'smb_rpc::tests::final_rpc_requests::bounded_original_requests'
TAIL_FILTER = 'smb_rpc::tests::final_rpc_requests::bounded_write_tails'
ROOT = Path(__file__).resolve().parents[2]
GIB = 1024 ** 3
HERE = Path(__file__).resolve().parent
WIRING = b'\n[[bin]]\nname="final_rpc_seed"\npath="../../test/engine/bench/final_rpc_seed.rs"\ntest=false\nbench=false\n'
TEST_WIRING = b'\n#[path = "../../bench/final_rpc_requests.rs"]\nmod final_rpc_requests;\n'
TRACKED = ['src/engine/Cargo.toml', 'test/server/unit/smb_rpc/tests.rs']
ADDED = ['test/engine/bench/final_rpc_seed.rs', 'test/server/bench/final_rpc_requests.rs',
         'test/server/bench/final_rpc_read.rs']

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
    tracked_benches = {path for path in ADDED if git(root, 'ls-files', '--', path)}
    assert not git(root, 'diff', '--', *ADDED), 'tracked bench bytes changed'
    assert set(git(root, 'ls-files', '--others', '--exclude-standard').splitlines()) == set(ADDED) - tracked_benches
    changes = {}
    for path in TRACKED + ADDED:
        before = subprocess.check_output(['git', '-C', str(root), 'show', 'HEAD:' + path]) if path in TRACKED or path in tracked_benches else None
        changes[path] = dict(before_sha256=hashlib.sha256(before).hexdigest() if before is not None else None,
                             after_sha256=sha(root / path))
    return dict(product_sha=git(root, 'rev-parse', 'HEAD'), base_tree=git(root, 'rev-parse', 'HEAD^{tree}'),
                effective_test_build_tree=effective_tree(root), overlay=changes,
                production_source_changes=False)

def prepare(workspace, evidence):
    assert git(ROOT, 'rev-parse', 'HEAD') == os.environ['RPC_SOURCE_SHA']
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
        for path in ADDED:
            (root / path).parent.mkdir(parents=True, exist_ok=True)
        adapter = 'baseline' if label == 'baseline' else 'current'
        sources = [ROOT / ADDED[0], ROOT / ADDED[1], ROOT / f'test/server/bench/final_rpc_{adapter}_read.rs']
        for target, source in zip(ADDED, sources):
            if git(root, 'ls-files', '--', target):
                assert (root / target).read_bytes() == source.read_bytes(), 'tracked bench differs from authenticated controller'
        shutil.copyfile(ROOT / ADDED[0], root / ADDED[0])
        shutil.copyfile(ROOT / ADDED[1], root / ADDED[1])
        adapter = 'baseline' if label == 'baseline' else 'current'
        shutil.copyfile(ROOT / f'test/server/bench/final_rpc_{adapter}_read.rs', root / ADDED[2])
        identities[label] = identity(root)
    write(evidence / 'source-before.json', identities)
    write(evidence / 'controller.json', dict(sha=os.environ['GITHUB_SHA'], run_id=os.environ['GITHUB_RUN_ID'],
          normal_source_sha=git(ROOT, 'rev-parse', 'HEAD'), normal_source_tree=git(ROOT, 'rev-parse', 'HEAD^{tree}'),
          files={p.name:sha(p) for p in HERE.iterdir() if p.is_file()},
          source_pins=PINS,
          deadline_unix=int(time.time()) + 90 * 60,
          criteria=dict(read_list_p95_delta_ms=2, read_list_p99_delta_ms=5, save_batch_delta_ms=1000,
                        write_tail_p95_delta_ms=50, save_p95_delta_ms=50, open_vmhwm_reduction_220=0.20, startup_ratio_220=1.10),
          old_parity_gate='FAILED unchanged; independent prospective engineering screen',
          exclusions=['not user SLO','not capacity certification','not network SMB','not physical12disk'],
          protocol='500 files;22/220 seed commits;1 file branch;1000 read/list cycles;20 committed closes;then 40x8MiB Flush/append64KiB/Close + 40x8MiB write-through;AB/BA'))

def seal(workspace, evidence, bundle):
    before = json.loads((evidence / 'source-before.json').read_text())
    after = {label:identity(workspace / label) for label in PINS}
    assert before == after, 'build changed authenticated input'
    write(evidence / 'source-after.json', after)
    bundle.mkdir(parents=True, exist_ok=False)
    binaries = {}
    features = {}
    seed_features = {}
    compilers = {}
    for label in PINS:
        rows = [json.loads(line) for line in (evidence / f'{label}-server.jsonl').read_text().splitlines() if line.startswith('{')]
        features[label] = [dict(target=row['target']['name'], features=row['features'], profile=row['profile'])
            for row in rows if row.get('reason') == 'compiler-artifact'
            and row['target']['name'] in ('storage_engine','storage_server')]
        assert all(row['profile']['opt_level'] == '3' for row in features[label])
        compilers[label] = (evidence / f'{label}-rustc.txt').read_text().strip()
        assert compilers[label].startswith('rustc 1.99.0 ')
        seed_rows = [json.loads(line) for line in (evidence / f'{label}-seed.jsonl').read_text().splitlines() if line.startswith('{')]
        seed_features[label] = [dict(target=row['target']['name'], features=row['features'], profile=row['profile'])
            for row in seed_rows if row.get('reason') == 'compiler-artifact'
            and row['target']['name'] in ('storage_engine', 'final_rpc_seed')]
        assert len(seed_features[label]) == 2
        assert all(row['features'] == ['fault-injection'] and row['profile']['opt_level'] == '3' for row in seed_features[label])
        engine_features = [row['features'] for row in features[label] if row['target'] == 'storage_engine']
        assert engine_features == [['fault-injection']], engine_features
        matches = [row['executable'] for row in rows if row.get('reason') == 'compiler-artifact'
                   and row.get('executable') and row['target']['name'] == 'storage_server'
                   and row['profile']['test']]
        assert len(matches) == 1, matches
        seed = Path(os.environ['RUNNER_TEMP']) / f'rpc-target-{label}/x86_64-unknown-linux-gnu/release/final_rpc_seed'
        for kind, source in (('seed', seed), ('server', Path(matches[0]))):
            destination = bundle / f'{label}-{kind}'
            shutil.copyfile(source, destination)
            destination.chmod(0o755)
            binaries[destination.name] = sha(destination)
    assert compilers['baseline'] == compilers['candidate']
    manifest = dict(source=after, binaries=binaries, build_features=features, seed_features=seed_features, compilers=compilers,
                    build_scope='release server lib-test, existing dev-dependency engine features; no failpoints or test-support helpers activated',
                    controller=json.loads((evidence / 'controller.json').read_text()))
    write(bundle / 'manifest.json', manifest)
    shutil.copyfile(bundle / 'manifest.json', evidence / 'manifest.json')
    print('MANIFEST_SHA256=' + sha(bundle / 'manifest.json'))

def limit():
    resource.setrlimit(resource.RLIMIT_AS, (8*GIB, 8*GIB))
    cpus = sorted(os.sched_getaffinity(0))
    assert len(cpus) >= 2, 'two CPUs required'
    os.sched_setaffinity(0, cpus[:2])

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
    if name in ('requests', 'tails'):
        values = [json.loads(line.split('RPC_RESULT ', 1)[1]) for line in raw.splitlines() if 'RPC_RESULT ' in line]
        assert len(values) == 1 and '1 passed; 0 failed' in raw, raw[-3000:]
        return values[0]
    values = [json.loads(line) for line in raw.splitlines() if line.startswith('{')]
    assert len(values) == 1, raw[-3000:]
    return values[0]

REUSE = dict(run_id=41166634669, carrier_sha='dab1b15d99122a59655a80ad1e2c28b626f97748',
             source_sha='70ff4929252b24c6f4b53719940cbec127b77561',
             source_tree='7ec07a6323a7b2244c0adc7135f35e56169752c5',
             manifest_sha256='bc2cc8e53b796d8aa6fb2bd9fb493133b15445e5e764aa5cb166d199b8975700',
             build_job=124437082759)
CONTROLLER_PATCHES = {'tools/local-rpc-acceptance/screen.py',
                      'tools/local-rpc-acceptance/reuse-workflow.yml',
                      'test/tools/local-rpc-acceptance/test_screen.py',
                      'docs/validation/final-local-rpc-acceptance.md',
                      'tools/local-rpc-acceptance/budget_contract.py',
                      'tools/local-rpc-acceptance/original-220-workflow.yml',
                      'test/tools/local-rpc-acceptance/test_budget_contract.py'}


def read_observation(path):
    """An absent cgroup control is observable, never an inferred unlimited budget."""
    try:
        return dict(status='present', path=str(path), raw=Path(path).read_text())
    except FileNotFoundError as error:
        return dict(status='not_present', path=str(path), errno=error.errno)


def host_observation():
    affinity = sorted(os.sched_getaffinity(0))
    assert len(affinity) >= 2, 'two CPUs required'
    return dict(uname=list(os.uname()), cpu=Path('/proc/cpuinfo').read_text(),
                memory=Path('/proc/meminfo').read_text(), affinity=affinity, child_cpus=affinity[:2],
                proc_cgroup=Path('/proc/self/cgroup').read_text(),
                cpu_max=read_observation('/sys/fs/cgroup/cpu.max'),
                memory_max=read_observation('/sys/fs/cgroup/memory.max'))


def validate_reuse(manifest):
    controller = manifest['controller']
    assert controller['sha'] == REUSE['carrier_sha']
    assert controller['run_id'] == str(REUSE['run_id'])
    assert controller['normal_source_sha'] == REUSE['source_sha']
    assert controller['normal_source_tree'] == REUSE['source_tree']
    assert controller['source_pins'] == {k:list(v) for k,v in PINS.items()}
    assert git(ROOT, 'rev-parse', 'HEAD') == os.environ['RPC_SOURCE_SHA']
    changes = set(git(ROOT, 'diff', '--name-only', REUSE['source_sha'], 'HEAD').splitlines())
    assert changes and changes <= CONTROLLER_PATCHES, changes
    # Authenticate exactly the compiled harness, not just its product source.
    for label in PINS:
        overlay = manifest['source'][label]['overlay']
        for path in ADDED[:2]:
            assert sha(ROOT / path) == overlay[path]['after_sha256'], path
        adapter = 'baseline' if label == 'baseline' else 'current'
        assert sha(ROOT / f'test/server/bench/final_rpc_{adapter}_read.rs') == overlay[ADDED[2]]['after_sha256']


def reuse(evidence):
    manifest_path = evidence / 'manifest.json'
    assert sha(manifest_path) == REUSE['manifest_sha256']
    manifest = json.loads(manifest_path.read_text())
    validate_reuse(manifest)
    run = json.loads((evidence / 'reuse-run.json').read_text())
    jobs = json.loads((evidence / 'reuse-jobs.json').read_text())['jobs']
    assert run['id'] == REUSE['run_id'] and run['head_sha'] == REUSE['carrier_sha']
    assert run['event'] == 'workflow_dispatch' and run['status'] == 'completed'
    assert run['repository']['full_name'] == 'isarmg/zxs'
    build = [job for job in jobs if job['id'] == REUSE['build_job']]
    assert len(build) == 1 and build[0]['conclusion'] == 'success'
    assert manifest['source'] == json.loads((evidence / 'source-after.json').read_text())
    assert json.loads((evidence / 'source-before.json').read_text()) == manifest['source']
    print('REUSED_BUILD', json.dumps(dict(**REUSE, controller_sha=os.environ['GITHUB_SHA'],
          normal_source_sha=os.environ['RPC_SOURCE_SHA'], original_deadline_unix=manifest['controller']['deadline_unix'])))

def run(bundle, out, commits, pair, *, original_only=False, execution_deadline=None):
    assert os.environ.get('GITHUB_ACTIONS') == 'true', 'workloads require GitHub Actions'
    assert commits in (22,220) and pair in (0,1)
    if original_only:
        assert commits == 220 and execution_deadline is not None
    else:
        assert execution_deadline is None
    manifest = json.loads((bundle / 'manifest.json').read_text())
    assert sha(bundle / 'manifest.json') == os.environ['RPC_MANIFEST_SHA256']
    if os.environ.get('RPC_REUSE_BUILD') == str(REUSE['run_id']):
        assert sha(bundle / 'manifest.json') == REUSE['manifest_sha256']
        validate_reuse(manifest)
    else:
        assert not os.environ.get('RPC_REUSE_BUILD'), 'unsupported build reuse'
        assert manifest['controller']['sha'] == os.environ['GITHUB_SHA']
        assert manifest['controller']['normal_source_sha'] == os.environ['RPC_SOURCE_SHA'] == git(ROOT, 'rev-parse', 'HEAD')
        assert manifest['controller']['normal_source_tree'] == git(ROOT, 'rev-parse', 'HEAD^{tree}')
    assert manifest['controller']['source_pins'] == {k:list(v) for k,v in PINS.items()}
    assert set(manifest['binaries']) == {label+'-'+kind for label in PINS for kind in ('seed','server')}
    for name, digest in manifest['binaries'].items():
        assert sha(bundle / name) == digest
        (bundle / name).chmod(0o755)
    out.mkdir(parents=True, exist_ok=False)
    report = dict(commits=commits, pair=pair, order=['baseline','candidate'] if pair == 0 else ['candidate','baseline'],
                  status='incomplete', manifest_sha256=sha(bundle / 'manifest.json'), runs=[],
                  profile='original-220' if original_only else 'full',
                  host={}, controller_sha=os.environ['GITHUB_SHA'],
                  normal_source_sha=os.environ['RPC_SOURCE_SHA'], reused_build=os.environ.get('RPC_REUSE_BUILD'),
                  checkpoint_selection='not observable via existing public API; warnings recorded',
                  caveat='pre-request full verification/inventory may warm OS caches; not cold device')
    try:
        report['host'] = host_observation()
        for label in report['order']:
            arm = out / label
            arm.mkdir()
            record = dict(label=label, status='incomplete', phases={})
            report['runs'].append(record)
            with tempfile.TemporaryDirectory(prefix='rpc-', dir=os.environ['RUNNER_TEMP']) as tmp:
                root = Path(tmp)
                env = dict(os.environ, ZXS_RPC_COMMITS=str(commits), ZXS_RPC_ROOT=tmp,
                           RAYON_NUM_THREADS='2', TOKIO_WORKER_THREADS='2')
                phases = ('build','edit','verify','requests','rpc-verify')
                if not original_only:
                    phases += ('tails','tails-verify')
                for name in phases:
                    command = [str(bundle / f'{label}-server'), '--ignored', '--exact', FILTER if name == 'requests' else TAIL_FILTER, '--nocapture', '--test-threads=1'] if name in ('requests', 'tails') else [str(bundle / f'{label}-seed'), name, '500', tmp]
                    record['phases'][name] = phase(command, env, arm, name, execution_deadline if original_only else manifest['controller']['deadline_unix'])
                    if name in ('verify','rpc-verify','tails-verify'):
                        record.setdefault('inventory', {})[name] = dict(metadata=inventory(root/'d1/metadata'),
                            immutable_objects=inventory(root/'d1/metadata/objects'), state=inventory(root/'state'))
                    if name in ('build', 'edit'):
                        shutil.copyfile(root/'operations', arm/'seed-operation-ids.txt')
                    elif name == 'requests':
                        shutil.copyfile(root/'rpc-receipts.json', arm/'rpc-receipts.json')
                    elif name == 'tails':
                        shutil.copyfile(root/'tails-receipts.json', arm/'tails-receipts.json')
                    write(out / 'pair.json', report)
            record['status'] = 'completed'
        report['status'] = 'completed'
    except BaseException:
        report['error'] = traceback.format_exc()
        raise
    finally:
        write(out / 'pair.json', report)
        print('RPC_PAIR', json.dumps(dict(status=report['status'], commits=commits, pair=pair, order=report['order'])))

def quantile(values, p):
    return sorted(values)[math.ceil(len(values)*p)-1]

def samples(value, count):
    assert isinstance(value, list) and len(value) == count, 'incomplete sample series'
    assert all(type(item) is int and item >= 0 for item in value), 'invalid nanoseconds'
    return value


def distribution(values):
    return {name: quantile(values, p) for name, p in [('p50', .5), ('p95', .95), ('p99', .99), ('max', 1)]}


def delta_check(baseline, candidate, limit):
    return dict(baseline_ns=baseline, candidate_ns=candidate, delta_ns=candidate-baseline,
                limit_ns=limit, passed=candidate-baseline <= limit)


def validate_original_phases(phases, commits):
    oracle = phases['verify']
    for key, value in dict(verified_current_files=500, verified_operations=commits,
                          verified_commit_namespaces=commits, verified_bodies=500+commits-2).items():
        assert oracle[key] == value
    after = phases['rpc-verify']
    assert after['sequence'] == commits+20 and after['current_files'] == 500
    assert after['saved_receipts'] == after['saved_commit_namespaces'] == after['saved_historical_bodies'] == 20
    requests = phases['requests']
    assert requests['schema'] == 'final-local-rpc-v1' and requests['commits_before'] == commits
    assert requests['verified_reads'] == 1001 and requests['verified_lists'] == 1000 and requests['verified_receipts'] == 20
    for key in ('read_ns', 'read_open_ns', 'list_ns'):
        samples(requests[key], 1000)
    samples(requests['save_ns'], 20)


def validate_arm(arm, commits):
    assert arm['status'] == 'completed'
    phases = arm['phases']
    assert set(phases) == {'build', 'edit', 'verify', 'requests', 'rpc-verify', 'tails', 'tails-verify'}
    validate_original_phases(phases, commits)
    tails = phases['tails']
    assert tails['schema'] == 'final-local-rpc-tails-v1' and tails['commits_before'] == commits+20
    assert tails['samples_per_metric'] == 40 and tails['published_generations'] == 120
    assert tails['body_bytes'] == 8*1024*1024 and tails['append_bytes'] == 64*1024
    for key in ('flush_ns', 'close_ns', 'write_through_ns'):
        samples(tails[key], 40)
    after = phases['tails-verify']
    assert after['sequence'] == commits+140
    assert after['saved_receipts'] == after['saved_historical_bodies'] == 120
    assert after['current_tail_files'] == 80 and after['independent_full_copies'] == 1


def original_checks(arms, commits):
    b, c = [arms[label]['phases']['requests'] for label in PINS]
    checks = {}
    for kind in ('read', 'list'):
        for percentile, tolerance in ((.95, 2_000_000), (.99, 5_000_000)):
            checks[f'{kind}_p{int(percentile*100)}'] = delta_check(
                quantile(b[kind+'_ns'], percentile), quantile(c[kind+'_ns'], percentile), tolerance)
    checks['save_batch'] = delta_check(b['save_batch_ns'], c['save_batch_ns'], 1_000_000_000)
    checks['save_p95'] = delta_check(quantile(b['save_ns'], .95), quantile(c['save_ns'], .95), 50_000_000)
    if commits == 220:
        checks['open_vmhwm'] = dict(baseline_kib=b['open_memory']['vmhwm_kib'], candidate_kib=c['open_memory']['vmhwm_kib'],
            ratio_limit=.8, passed=c['open_memory']['vmhwm_kib'] <= .8*b['open_memory']['vmhwm_kib'])
        checks['startup'] = dict(baseline_ns=b['open_ns'], candidate_ns=c['open_ns'], ratio_limit=1.1,
            passed=c['open_ns'] <= 1.1*b['open_ns'])
    return checks


def compare_pair(data, commits, pair, manifest_hash):
    assert data['status'] == 'completed' and data['commits'] == commits and data['pair'] == pair
    assert data['manifest_sha256'] == manifest_hash
    order = ['baseline', 'candidate'] if pair == 0 else ['candidate', 'baseline']
    assert data['order'] == order and [arm['label'] for arm in data['runs']] == order
    assert len(data['host']['child_cpus']) == 2
    arms = {arm['label']: arm for arm in data['runs']}
    for arm in arms.values():
        validate_arm(arm, commits)
    checks = original_checks(arms, commits)
    tails = {label: arms[label]['phases']['tails'] for label in PINS}
    for kind in ('flush', 'close', 'write_through'):
        checks[kind+'_p95'] = delta_check(quantile(tails['baseline'][kind+'_ns'], .95),
                                        quantile(tails['candidate'][kind+'_ns'], .95), 50_000_000)
    return dict(commits=commits, pair=pair, checks=checks,
        distributions={label: {kind: distribution(arms[label]['phases']['requests'][kind+'_ns'])
                       for kind in ('read', 'list', 'save')} for label in PINS},
        write_tail_distributions={label: {kind: distribution(tails[label][kind+'_ns'])
                                  for kind in ('flush', 'close', 'write_through')} for label in PINS},
        metadata={label: arms[label]['inventory'] for label in PINS},
        warnings={label: arms[label]['phases']['requests']['recovery_warnings'] for label in PINS})


def summary(root, out):
    report = dict(status='incomplete', source_pins=PINS, old_parity_gate='FAILED unchanged',
                  prospective_screen='NOT EVALUATED', pairs=[])
    try:
        for commits in (22, 220):
            for pair in (0, 1):
                data = json.loads((root / f'rpc-{commits}-{pair}' / 'pair.json').read_text())
                report['pairs'].append(compare_pair(data, commits, pair, os.environ['RPC_MANIFEST_SHA256']))
        report.update(status='completed', prospective_screen='PASS' if all(
            check['passed'] for pair in report['pairs'] for check in pair['checks'].values()) else 'FAIL')
    finally:
        write(out, report)
        print('RPC_SCREEN', json.dumps(report))
    if report['prospective_screen'] != 'PASS':
        raise SystemExit(1)


REUSED_22 = {'rpc-22-0':'74efc7ee593ca4fb4929721353e59597f1af1ea8d6af6aa85683f9d8353f9092',
             'rpc-22-1':'1f4924a3b5f66e1039b73965c900316a0448815031952eb1a40b07400c149f7c'}


def budget_proof():
    return json.loads(subprocess.check_output([sys.executable, str(HERE/'budget_contract.py')], text=True))


def select_original_220(evidence, output):
    reuse(evidence)
    prior = []
    for pair, name in enumerate(REUSED_22):
        path = evidence/name/'pair.json'
        assert sha(path) == REUSED_22[name]
        prior.append(compare_pair(json.loads(path.read_text()), 22, pair, REUSE['manifest_sha256']))
    now = int(time.time())
    plan = dict(profile='original-220', controller_sha=os.environ['GITHUB_SHA'],
                normal_source_sha=os.environ['RPC_SOURCE_SHA'], created_unix=now,
                deadline_unix=now+45*60, per_pair_budget_seconds=45*60,
                original_phase_budget_seconds=1200, reused_22=prior, budget_proof=budget_proof(),
                old_run=41168852801, old_result='incomplete / NOT EVALUATED retained',
                full_extension='FAIL_DEFAULT_BUDGET; never replaced by the original-profile verdict')
    write(output, plan)
    print('ORIGINAL_220_PLAN', json.dumps(plan))


def run_original_220(bundle, out, pair, plan_path):
    assert sha(plan_path) == os.environ['RPC_PLAN_SHA256']
    plan = json.loads(plan_path.read_text())
    assert plan['profile'] == 'original-220'
    assert plan['controller_sha'] == os.environ['GITHUB_SHA']
    assert plan['normal_source_sha'] == os.environ['RPC_SOURCE_SHA']
    assert plan['deadline_unix']-plan['created_unix'] == 45*60
    assert plan['created_unix'] <= time.time() < plan['deadline_unix']
    run(bundle, out, 220, pair, original_only=True, execution_deadline=plan['deadline_unix'])


def compare_original_pair(data, pair):
    assert data['profile'] == 'original-220' and data['status'] == 'completed'
    assert data['commits'] == 220 and data['pair'] == pair
    assert data['manifest_sha256'] == REUSE['manifest_sha256']
    order = ['baseline','candidate'] if pair == 0 else ['candidate','baseline']
    assert data['order'] == order and [arm['label'] for arm in data['runs']] == order
    assert len(data['host']['child_cpus']) == 2
    arms = {arm['label']:arm for arm in data['runs']}
    for arm in arms.values():
        assert arm['status'] == 'completed'
        assert set(arm['phases']) == {'build','edit','verify','requests','rpc-verify'}
        validate_original_phases(arm['phases'], 220)
    return dict(commits=220, pair=pair, checks=original_checks(arms, 220))


def summary_original_220(root, plan_path, out):
    assert sha(plan_path) == os.environ['RPC_PLAN_SHA256']
    plan = json.loads(plan_path.read_text())
    assert plan['profile'] == 'original-220'
    assert plan['controller_sha'] == os.environ['GITHUB_SHA']
    assert plan['normal_source_sha'] == os.environ['RPC_SOURCE_SHA']
    report = dict(status='incomplete', actual_product_pins=PINS, old_run=41168852801,
                  old_result='incomplete / NOT EVALUATED retained', old_2af_gate='FAIL unchanged',
                  original_220_gate='NOT EVALUATED', continuous_extension_support='FAIL_DEFAULT_BUDGET',
                  full_extension_acceptance='FAIL', budget_proof=budget_proof(), reused_22=plan['reused_22'], pairs=[])
    try:
        for pair in (0,1):
            data = json.loads((root/f'rpc-220-original-{pair}'/'pair.json').read_text())
            report['pairs'].append(compare_original_pair(data, pair))
        passed = all(check['passed'] for pair in report['pairs'] for check in pair['checks'].values())
        report.update(status='completed', original_220_gate='PASS' if passed else 'FAIL')
    finally:
        write(out, report)
        print('ORIGINAL_220_SCREEN', json.dumps(report))
    if report['original_220_gate'] != 'PASS' or report['full_extension_acceptance'] != 'PASS':
        raise SystemExit(1)

if __name__ == '__main__':
    action, *args = sys.argv[1:]
    if action == 'prepare': prepare(*map(Path,args))
    elif action == 'seal': seal(*map(Path,args))
    elif action == 'reuse': reuse(*map(Path,args))
    elif action == 'select-original': select_original_220(*map(Path,args))
    elif action == 'run-original': run_original_220(Path(args[0]),Path(args[1]),int(args[2]),Path(args[3]))
    elif action == 'summary-original': summary_original_220(*map(Path,args))
    elif action == 'run': run(Path(args[0]),Path(args[1]),int(args[2]),int(args[3]))
    elif action == 'summary': summary(*map(Path,args))
    else: raise ValueError('unsupported action')
