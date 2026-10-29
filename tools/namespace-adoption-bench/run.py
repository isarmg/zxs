#!/usr/bin/env python3
"""Verification-only paired runner. It never changes a product checkout."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import resource
import shutil
import signal
import subprocess
import tempfile
import time
import traceback

from artifacts import verify_bundle

GIB = 1024 ** 3
PHASES = ('build', 'edit', 'read', 'open', 'verify')


def limits():
    # A benchmark child is an external workload with finite virtual address space.
    resource.setrlimit(resource.RLIMIT_AS, (8 * GIB, 8 * GIB))


def allocation_size(root):
    apparent = allocated = objects = 0
    for directory, _, names in os.walk(root):
        for name in names:
            info = (Path(directory) / name).stat()
            apparent += info.st_size
            allocated += info.st_blocks * 512
            objects += 1
    return dict(apparent_bytes=apparent, allocated_bytes=allocated, files=objects)


def stop_process_group(child):
    # Only this owned measurement and its optional strace child belong to the group.
    try:
        os.killpg(child.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        child.wait(timeout=10)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        child.wait()


def run_phase(binary, phase, files, pool, evidence, traced, global_deadline):
    command = [str(binary), phase, str(files), str(pool)]
    if traced:
        command = ['strace', '-f', '-c', '-e',
                   'trace=fsync,fdatasync,rename,renameat,renameat2,mkdir,mkdirat,unlink,unlinkat',
                   '-o', str(evidence / f'{phase}.strace'), *command]
    record = dict(phase=phase, command=command, traced=traced,
                  started_unix_ns=time.time_ns(), timeout_seconds=1200,
                  minimum_free_bytes=8 * GIB, address_space_limit_bytes=8 * GIB,
                  status='failed', reason='launch_not_completed')
    record_path = evidence / f'{phase}.json'
    if time.time() >= global_deadline:
        record.update(status='incomplete', reason='global_time_budget')
        record_path.write_text(json.dumps(record, indent=2) + '\n')
        return record
    record_path.write_text(json.dumps(record, indent=2) + '\n')
    with (evidence / f'{phase}.stdout').open('wb') as stdout, (evidence / f'{phase}.stderr').open('wb') as stderr:
        child = subprocess.Popen(command, stdout=stdout, stderr=stderr,
                                 start_new_session=True, preexec_fn=limits)
        try:
            deadline = time.monotonic() + 1200
            record.update(pid=child.pid, status='running', reason='process_not_completed')
            record_path.write_text(json.dumps(record, indent=2) + '\n')
            reason = None
            while child.poll() is None:
                if time.time() >= global_deadline:
                    reason = 'global_time_budget'
                elif time.monotonic() >= deadline:
                    reason = 'phase_time_budget'
                elif shutil.disk_usage(pool).free < 8 * GIB:
                    reason = 'runner_free_space_budget'
                if reason is not None:
                    stop_process_group(child)
                    break
                time.sleep(0.5)
            record.update(returncode=child.wait(), finished_unix_ns=time.time_ns())
        finally:
            if child.poll() is None:
                stop_process_group(child)
    if reason is not None:
        record.update(status='incomplete', reason=reason)
    elif record['returncode']:
        record.update(status='failed', reason='benchmark_process_failed')
    else:
        record.update(status='failed', reason='output_not_validated')
        record_path.write_text(json.dumps(record, indent=2) + '\n')
        rows = [json.loads(line) for line in (evidence / f'{phase}.stdout').read_text().splitlines()]
        expected = ['read_after_open', 'read_warm_repeat'] if phase == 'read' else [phase]
        if [row['phase'] for row in rows] != expected or any(row['files'] != files for row in rows):
            raise RuntimeError(f'invalid benchmark output in {evidence / phase}')
        record.update(status='completed', reason=None, measurements=rows)
    record_path.write_text(json.dumps(record, indent=2) + '\n')
    return record


def write_json(path, value):
    temporary = path.with_suffix('.tmp')
    temporary.write_text(json.dumps(value, indent=2) + '\n')
    temporary.replace(path)


def pair_order(repetition):
    return ('baseline', 'candidate') if repetition % 2 == 0 else ('candidate', 'baseline')


def planned_runs(sizes, repetitions):
    return [dict(name=f'{size}-{repetition}-{implementation}', files=size,
                 repetition=repetition, implementation=implementation,
                 traced=repetition == 3, phases=[], attempted_phases=[], status='not_run',
                 not_run_phases=list(PHASES))
            for size in sizes for repetition in repetitions
            for implementation in pair_order(repetition)]


def measure_implementation(run, binary, args, checkpoint):
    evidence = args.output / run['name']
    evidence.mkdir()
    run.update(started_unix_ns=time.time_ns(), status='running')
    checkpoint()
    try:
        if time.time() >= args.deadline_unix:
            run.update(status='incomplete', reason='global_time_budget')
            return
        size = run['files']
        required = ({500: 2, 1500: 6, 3000: 16}[size] + 8) * GIB
        available = shutil.disk_usage(args.pool_parent).free
        if available < required:
            run.update(status='incomplete', reason='pool_space_preflight',
                       required_free_bytes=required, observed_free_bytes=available)
            return
        # Every implementation/repetition owns a new pool, including after a failed counterpart.
        with tempfile.TemporaryDirectory(prefix='owned-task1-', dir=args.pool_parent) as temporary:
            pool = Path(temporary)
            for index, phase in enumerate(PHASES):
                run['active_phase'] = phase
                run['attempted_phases'].append(phase)
                run['not_run_phases'] = list(PHASES[index + 1:])
                checkpoint()
                result = run_phase(binary, phase, size, pool, evidence,
                                   run['traced'], args.deadline_unix)
                run['phases'].append(result)
                checkpoint()
                if result['status'] != 'completed':
                    run.update(status=result['status'], reason=result['reason'])
                    return
            run.update(status='completed', reason=None, pool=allocation_size(pool))
    except (OSError, ValueError, KeyError, TypeError, RuntimeError) as error:
        # Process/filesystem/structured-output failures are the runner's external boundary.
        run.update(status='failed', reason='measurement_boundary_failed',
                   error_type=type(error).__name__, error=str(error))
        (evidence / 'measurement.error.txt').write_text(traceback.format_exc())
    finally:
        run.pop('active_phase', None)
        run['finished_unix_ns'] = time.time_ns()
        write_json(evidence / 'run.json', run)
        checkpoint()


def parse_arguments():
    parser = argparse.ArgumentParser()
    parser.add_argument('--bundle-dir', type=Path, required=True,
                        help='build-once bundle; its manifest and binaries are verified before execution')
    parser.add_argument('--audit-dir', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--pool-parent', type=Path, required=True)
    parser.add_argument('--deadline-unix', type=int, required=True)
    parser.add_argument('--sizes', type=int, nargs='+', default=[500, 1500, 3000])
    parser.add_argument('--pair-index', type=int, choices=range(4),
                        help='0: untraced AB; 1: untraced BA; 2: untraced AB; 3: traced BA')
    args = parser.parse_args()
    args.bundle_dir = args.bundle_dir.resolve()
    args.audit_dir = args.audit_dir.resolve()
    if not args.sizes or any(size not in (500, 1500, 3000) for size in args.sizes):
        parser.error('sizes must be drawn from 500, 1500, 3000')
    if len(set(args.sizes)) != len(args.sizes):
        parser.error('sizes must be unique')
    return args


def main():
    args = parse_arguments()
    args.output.mkdir(parents=True, exist_ok=False)
    args.pool_parent.mkdir(parents=True, exist_ok=True)
    repetitions = list(range(4)) if args.pair_index is None else [args.pair_index]
    outcome = dict(status='incomplete', runs=planned_runs(args.sizes, repetitions),
                   deadline_unix=args.deadline_unix, sizes=args.sizes,
                   pair_index=args.pair_index,
                   files=args.sizes[0] if len(args.sizes) == 1 else None,
                   order=list(pair_order(args.pair_index)) if args.pair_index is not None else None,
                   github_run_id=os.environ.get('GITHUB_RUN_ID'),
                   github_run_attempt=os.environ.get('GITHUB_RUN_ATTEMPT'),
                   carrier_sha=os.environ.get('GITHUB_SHA'),
                   requested_pair_indices=repetitions,
                   pair_orders={str(index): list(pair_order(index)) for index in repetitions},
                   full_suite=dict(sizes=[500, 1500, 3000], pair_indices=[0, 1, 2, 3]),
                   fixture=dict(directories=97, initial_files='N', saves=20,
                                permanent_results=22, unique_content_versions='N+20',
                                members=['P01'], target='target1'),
                   scope='requested same-runner paired task1 load; full suite requires all 12 pairs; not an adoption verdict',
                   unmeasured=['allocator requests/peak live heap', 'daemon lock wait/hold',
                               'true cold OS page cache', '8MiB namespace-cache eviction'])

    def checkpoint():
        write_json(args.output / 'summary.json', outcome)

    checkpoint()
    try:
        adapter_root = Path(__file__).resolve().parents[2] / 'test/engine/bench/namespace_adoption'
        binaries = verify_bundle(args.bundle_dir,
                                 {name: adapter_root / f'{name}.rs' for name in ('baseline', 'candidate')},
                                 args.audit_dir)
        audit_path = args.audit_dir / 'identity-verification.json'
        outcome['artifact_identity'] = json.loads(audit_path.read_text())
        outcome['artifact_manifest_sha256'] = hashlib.sha256(
            (args.bundle_dir / 'manifest.json').read_bytes()).hexdigest()
        outcome['binary_sha256'] = {name: hashlib.sha256(binary.read_bytes()).hexdigest()
                                    for name, binary in binaries.items()}
        for run in outcome['runs']:
            run['binary_sha256'] = outcome['binary_sha256'][run['implementation']]
        checkpoint()
        for run in outcome['runs']:
            # A failed or incomplete implementation must not hide the other half of its pair.
            measure_implementation(run, binaries[run['implementation']], args, checkpoint)
        statuses = [run['status'] for run in outcome['runs']]
        outcome['status_counts'] = {status: statuses.count(status) for status in sorted(set(statuses))}
        if all(status == 'completed' for status in statuses):
            outcome['status'] = 'completed_measurements'
            return 0
        outcome['status'] = 'failed' if 'failed' in statuses else 'incomplete'
        return 1 if outcome['status'] == 'failed' else 2
    except (OSError, ValueError, KeyError, TypeError, RuntimeError, subprocess.SubprocessError) as error:
        outcome.update(status='failed', reason='artifact_identity_or_evidence_failed',
                       error_type=type(error).__name__, error=str(error))
        (args.output / 'runner.error.txt').write_text(traceback.format_exc())
        return 1
    finally:
        checkpoint()


if __name__ == '__main__':
    raise SystemExit(main())
