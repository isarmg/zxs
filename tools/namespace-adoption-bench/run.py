#!/usr/bin/env python3
"""Verification-only paired runner. It never changes a product checkout."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import resource
import shutil
import signal
import subprocess
import tempfile
import time
import traceback

from artifacts import load_inputs, verify_bundle

GIB = 1024 ** 3
PHASES = ('build', 'edit', 'read', 'open', 'verify')
TRACE_SYSCALLS = ('fsync', 'fdatasync', 'rename', 'renameat', 'renameat2',
                  'mkdir', 'mkdirat', 'unlink', 'unlinkat')


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


def ambient_observer_status(status_path=Path('/proc/self/status')):
    fields = {name: None for name in ('NoNewPrivs', 'Seccomp', 'Seccomp_filters')}
    observation = dict(source=str(status_path), fields=fields, raw_fields=[],
                       observed_unix_ns=time.time_ns(), unfiltered=False)
    try:
        lines = status_path.read_text().splitlines()
    except OSError as error:
        observation.update(error_type=type(error).__name__, error=str(error))
        return observation
    for name in fields:
        matches = [line for line in lines if line.partition(':')[0] == name]
        observation['raw_fields'].extend(matches)
        if len(matches) == 1 and matches[0].partition(':')[2].strip().isdecimal():
            fields[name] = int(matches[0].partition(':')[2].strip())
    observation['unfiltered'] = (fields['NoNewPrivs'] in (0, 1) and
                                 fields['Seccomp'] == 0 and fields['Seccomp_filters'] == 0)
    return observation


def verify_observer_probe(path):
    admission = dict(status='incomplete', path=str(path) if path is not None else None)

    def check(condition, reason):
        if not condition:
            raise ValueError(reason)

    try:
        check(path is not None, 'deterministic observer probe is required for BPF measurement')
        path = Path(path).resolve()
        admission['path'] = str(path)
        raw = path.read_bytes()
        admission['sha256'] = hashlib.sha256(raw).hexdigest()
        probe = json.loads(raw)
        expected = dict(schema='namespace-observer-probe-v1', status='completed_probe',
                        mode='observer-validation', scope='deterministic-9-syscall-contract-only',
                        github_run_id=os.environ['GITHUB_RUN_ID'],
                        github_run_attempt=os.environ['GITHUB_RUN_ATTEMPT'],
                        carrier_sha=os.environ['GITHUB_SHA'], github_job=os.environ['GITHUB_JOB'],
                        runner_name=os.environ['RUNNER_NAME'])
        for field, value in expected.items():
            check(probe[field] == value, f'observer probe {field} differs from this job/contract')
        check(probe['probe_script_sha256'] == hashlib.sha256(
            Path(__file__).with_name('observer_probe.py').read_bytes()).hexdigest(),
            'observer probe script differs from this controller')
        ambient = probe['ambient']['values']
        check(set(ambient) == {'NoNewPrivs', 'Seccomp', 'Seccomp_filters'} and
              ambient['NoNewPrivs'] in (0, 1) and ambient['Seccomp'] == 0 and
              ambient['Seccomp_filters'] == 0, 'observer probe ambient filter is unsupported')
        executable = shutil.which('strace')
        check(executable is not None, 'current strace executable is unavailable')
        executable = Path(executable).resolve()
        with executable.open('rb') as source:
            binary_sha256 = hashlib.file_digest(source, 'sha256').hexdigest()
        check(probe['strace']['path'] == str(executable) and
              probe['strace']['sha256'] == binary_sha256, 'observer probe strace executable differs')
        version = (path.parent / 'version.stdout').read_bytes()
        check(version and hashlib.sha256(version).hexdigest() == probe['strace']['version_sha256'] and
              version.decode() == probe['strace']['version'], 'observer probe strace version evidence differs')
        check([item['mode'] for item in probe['probes']] == ['plain', 'seccomp-bpf'],
              'observer probe must validate both plain and BPF modes')
        for item in probe['probes']:
            mode = item['mode']
            check(item['status'] == 'completed' and
                  item['counts'] == dict(syscalls={name: dict(calls=6, errors=3) for name in TRACE_SYSCALLS},
                                         total=dict(calls=54, errors=27)),
                  f'{mode}: deterministic nine-syscall counts/errors are not exact')
            process = item['process']
            observer = item['observer']
            check(process['returncode'] == 0 and process['timed_out'] is False,
                  f'{mode}: deterministic worker did not complete')
            check(observer['requested_mode'] == mode and observer['effective_mode'] == mode and
                  observer['syscalls'] == list(TRACE_SYSCALLS) and
                  observer['full_argv'] == process['command'], f'{mode}: observer command or mode differs')
            command = process['command']
            check([argument for argument in command if argument.startswith('trace=')] ==
                  ['trace=' + ','.join(TRACE_SYSCALLS)] and '-f' in command and '-c' in command and
                  ('--seccomp-bpf' in command) == (mode == 'seccomp-bpf') and
                  ('-d' in command) == (mode == 'seccomp-bpf'), f'{mode}: syscall collection flags differ')
            tracees = item['tracees']
            check(tracees['status'] == 'completed' and tracees['scope'] == 'deterministic-syscall-fixture-only' and
                  [worker['role'] for worker in tracees['workers']] == ['parent', 'fork-child', 'thread'],
                  f'{mode}: parent/fork/thread coverage is incomplete')
            for worker in tracees['workers']:
                for field in ('proc_self', 'proc_thread_self'):
                    values = worker[field]['values']
                    check(set(values) == {'NoNewPrivs', 'Seccomp', 'Seccomp_filters'} and
                          values['NoNewPrivs'] in (0, 1) and values['Seccomp'] == (2 if mode == 'seccomp-bpf' else 0) and
                          values['Seccomp_filters'] == (1 if mode == 'seccomp-bpf' else 0),
                          f'{mode}: tracee filter state is unsupported')
            if mode == 'seccomp-bpf':
                check(observer['activation_status'] == 'enabled_and_observed' and
                      observer['enabled_marker_count'] > 0 and observer['seccomp_event_count'] > 0 and
                      not observer['fallback_diagnostics'] and
                      {worker['tid'] for worker in tracees['workers']} <= set(observer['seccomp_event_pids']),
                      'BPF observer activation/parent/fork/thread events are not proven')
        admission.update(status='verified', strace=probe['strace'],
                         github_run_id=probe['github_run_id'], github_run_attempt=probe['github_run_attempt'],
                         carrier_sha=probe['carrier_sha'], github_job=probe['github_job'], runner_name=probe['runner_name'])
    except (OSError, KeyError, TypeError, ValueError) as error:
        admission.update(reason=str(error), error_type=type(error).__name__)
    return admission


def observer_evidence(stderr_path, requested, command):
    observation = dict(requested_mode=requested, effective_mode='plain' if requested == 'plain' else 'unknown',
                       full_argv=command, syscalls=list(TRACE_SYSCALLS),
                       enabled_marker_count=0, seccomp_event_count=0,
                       activation_evidence=[], fallback_diagnostics=[], seccomp_event_pids=[])
    with stderr_path.open('rb') as source:
        observation['stderr_sha256'] = hashlib.file_digest(source, 'sha256').hexdigest()
    fallback = re.compile(r'seccomp(?:-bpf)?\b.*(?:disabled|disabling|unavailable|not enabled)|no syscalls to filter', re.I)
    with stderr_path.open(errors='replace') as source:
        for line in source:
            enabled = 'seccomp filter enabled' in line
            event_match = re.search(r'\[wait\(0x[0-9a-fA-F]+\)\s*=\s*(\d+)\].*,EVENT_SECCOMP\s+\(7\)', line)
            event = event_match is not None
            if event and int(event_match.group(1)) not in observation['seccomp_event_pids']:
                observation['seccomp_event_pids'].append(int(event_match.group(1)))
            observation['enabled_marker_count'] += int(enabled)
            observation['seccomp_event_count'] += int(event)
            if (enabled or event) and len(observation['activation_evidence']) < 16:
                observation['activation_evidence'].append(line.rstrip())
            if fallback.search(line):
                observation['fallback_diagnostics'].append(line.rstrip())
    if requested == 'plain':
        observation['activation_status'] = 'plain_requested'
    elif (observation['enabled_marker_count'] and observation['seccomp_event_count']
          and not observation['fallback_diagnostics']):
        observation.update(effective_mode='seccomp-bpf', activation_status='enabled_and_observed')
    elif observation['fallback_diagnostics'] and not (observation['enabled_marker_count'] or observation['seccomp_event_count']):
        observation.update(effective_mode='plain', activation_status='fallback_to_plain')
    else:
        observation['activation_status'] = 'unproven_or_conflicting'
    return observation


def trace_command(binary_command, evidence, phase, observer):
    if observer not in ('plain', 'seccomp-bpf'):
        raise ValueError(f'unsupported trace observer: {observer}')
    # Debug output establishes real TRACESECCOMP events; trace timings are never timing evidence.
    options = ['--seccomp-bpf', '-d'] if observer == 'seccomp-bpf' else []
    return ['strace', '-f', *options, '-c', '-e', 'trace=' + ','.join(TRACE_SYSCALLS),
            '-o', str(evidence / f'{phase}.strace'), *binary_command]


def run_phase(binary, phase, files, pool, evidence, traced, global_deadline, trace_observer='plain'):
    command = [str(binary), phase, str(files), str(pool)]
    if traced:
        command = trace_command(command, evidence, phase, trace_observer)
    record = dict(phase=phase, command=command, traced=traced,
                  started_unix_ns=time.time_ns(), timeout_seconds=1200,
                  minimum_free_bytes=8 * GIB, address_space_limit_bytes=8 * GIB,
                  status='failed', reason='launch_not_completed')
    if traced:
        record['observer'] = dict(requested_mode=trace_observer, effective_mode='not_started',
                                  full_argv=command, syscalls=list(TRACE_SYSCALLS),
                                  activation_status='not_started')
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
    if traced:
        record['observer'] = observer_evidence(evidence / f'{phase}.stderr', trace_observer, command)
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
        if traced and trace_observer == 'seccomp-bpf' and record['observer']['activation_status'] != 'enabled_and_observed':
            record.update(status='incomplete', reason='trace_observer_activation_unproven')
    record_path.write_text(json.dumps(record, indent=2) + '\n')
    return record


def write_json(path, value):
    temporary = path.with_suffix('.tmp')
    temporary.write_text(json.dumps(value, indent=2) + '\n')
    temporary.replace(path)


def pair_order(repetition):
    return ('baseline', 'candidate') if repetition % 2 == 0 else ('candidate', 'baseline')


def planned_runs(sizes, repetitions, trace_observer='plain'):
    return [dict(name=f'{size}-{repetition}-{implementation}', files=size,
                 repetition=repetition, implementation=implementation,
                 traced=repetition == 3, trace_observer_requested=trace_observer if repetition == 3 else 'none',
                 phases=[], attempted_phases=[], status='not_run',
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
        if run['traced']:
            run['observer_preflight'] = ambient_observer_status()
            write_json(evidence / 'observer-preflight.json', run['observer_preflight'])
            if args.trace_observer == 'seccomp-bpf':
                run['observer_probe_admission'] = verify_observer_probe(args.observer_probe)
                write_json(evidence / 'observer-probe-admission.json', run['observer_probe_admission'])
                if not run['observer_preflight']['unfiltered']:
                    run.update(status='incomplete', reason='trace_observer_ambient_filter_unsupported')
                    return
                if run['observer_probe_admission']['status'] != 'verified':
                    run.update(status='incomplete', reason='trace_observer_probe_not_verified')
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
                                   run['traced'], args.deadline_unix, args.trace_observer)
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
    parser.add_argument('--mode', choices=('final', 'observer-validation'), required=True,
                        help='observer validation cannot satisfy final candidate acceptance')
    parser.add_argument('--trace-observer', choices=('plain', 'seccomp-bpf'), default='plain')
    parser.add_argument('--observer-probe', type=Path,
                        help='this job deterministic nine-syscall probe.json; required to admit BPF measurement')
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
    if args.mode == 'observer-validation' and (args.sizes != [500] or args.pair_index != 3):
        parser.error('observer-validation requires --sizes 500 --pair-index 3')
    return args


def main():
    args = parse_arguments()
    args.output.mkdir(parents=True, exist_ok=False)
    args.pool_parent.mkdir(parents=True, exist_ok=True)
    repetitions = list(range(4)) if args.pair_index is None else [args.pair_index]
    outcome = dict(status='incomplete', runs=planned_runs(args.sizes, repetitions, args.trace_observer),
                   deadline_unix=args.deadline_unix, sizes=args.sizes,
                   validation_mode=args.mode, trace_observer=args.trace_observer,
                   observer_probe=str(args.observer_probe) if args.observer_probe is not None else None,
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
                   scope=('500-file observer validation only; not final candidate acceptance'
                          if args.mode == 'observer-validation' else
                          'requested same-runner paired task1 load; full suite requires all 12 pairs; not an adoption verdict'),
                   unmeasured=['allocator requests/peak live heap', 'daemon lock wait/hold',
                               'true cold OS page cache', '8MiB namespace-cache eviction'])

    def checkpoint():
        write_json(args.output / 'summary.json', outcome)

    checkpoint()
    try:
        harness = Path(__file__).resolve().parents[2]
        selected_inputs = load_inputs(args.mode)
        binaries = verify_bundle(args.bundle_dir,
                                 {name: harness / identity['adapter_path']
                                  for name, identity in selected_inputs.items()},
                                 args.audit_dir, mode=args.mode)
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
