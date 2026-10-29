#!/usr/bin/env python3
"""Verification-only paired runner. It never changes a product checkout."""
import argparse
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
                   'trace=read,pread64,openat,openat2,newfstatat,fsync,fdatasync,rename,renameat,renameat2,mkdir,mkdirat,unlink,unlinkat',
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


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--baseline', type=Path, required=True)
    parser.add_argument('--candidate', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--pool-parent', type=Path, required=True)
    parser.add_argument('--deadline-unix', type=int, required=True)
    parser.add_argument('--sizes', type=int, nargs='+', default=[500, 1500, 3000])
    args = parser.parse_args()
    if not args.sizes or any(size not in (500, 1500, 3000) for size in args.sizes):
        parser.error('sizes must be drawn from 500, 1500, 3000')
    args.output.mkdir(parents=True, exist_ok=False)
    args.pool_parent.mkdir(parents=True, exist_ok=True)
    outcome = dict(status='incomplete', runs=[], deadline_unix=args.deadline_unix,
                   scope='same-runner paired task1 load, not an adoption verdict',
                   unmeasured=['allocator requests/peak live heap', 'daemon lock wait/hold',
                               'true cold OS page cache', '8MiB namespace-cache eviction'])
    def checkpoint():
        temporary = args.output / 'summary.tmp'
        temporary.write_text(json.dumps(outcome, indent=2) + '\n')
        temporary.replace(args.output / 'summary.json')
    checkpoint()
    try:
        for size in args.sizes:
            # Three timing repetitions; a separate trace repetition is never timing evidence.
            for repetition in range(4):
                traced = repetition == 3
                order = ('baseline', 'candidate') if repetition % 2 == 0 else ('candidate', 'baseline')
                for implementation in order:
                    name = f'{size}-{repetition}-{implementation}'
                    evidence = args.output / name
                    evidence.mkdir()
                    run = dict(name=name, files=size, repetition=repetition,
                               implementation=implementation, traced=traced, phases=[])
                    outcome['runs'].append(run)
                    checkpoint()
                    if time.time() >= args.deadline_unix:
                        run.update(status='incomplete', reason='global_time_budget')
                        checkpoint()
                        return 2
                    required = ({500: 2, 1500: 6, 3000: 16}[size] + 8) * GIB
                    available = shutil.disk_usage(args.pool_parent).free
                    if available < required:
                        run.update(status='incomplete', reason='pool_space_preflight', required_free_bytes=required, observed_free_bytes=available)
                        return 2
                    with tempfile.TemporaryDirectory(prefix='owned-task1-', dir=args.pool_parent) as temporary:
                        pool = Path(temporary)
                        for phase in PHASES:
                            run['active_phase'] = phase
                            checkpoint()
                            try:
                                result = run_phase(getattr(args, implementation), phase, size, pool, evidence, traced, args.deadline_unix)
                            except (OSError, ValueError, KeyError, TypeError, RuntimeError) as error:
                                # Process launch/files and external structured output are the runner boundary.
                                run.update(status='failed', error_type=type(error).__name__, error=str(error))
                                outcome['status'] = 'failed'
                                (evidence / f'{phase}.error.txt').write_text(traceback.format_exc())
                                checkpoint()
                                return 1
                            run['phases'].append(result)
                            checkpoint()
                            if result['status'] != 'completed':
                                run.update(status=result['status'])
                                outcome['status'] = result['status']
                                return 1 if result['status'] == 'failed' else 2
                        run.update(status='completed', pool=allocation_size(pool))
                    run.pop('active_phase')
                    checkpoint()
                    (evidence / 'run.json').write_text(json.dumps(run, indent=2) + '\n')
        outcome['status'] = 'completed_measurements'
        return 0
    finally:
        checkpoint()


if __name__ == '__main__':
    raise SystemExit(main())
