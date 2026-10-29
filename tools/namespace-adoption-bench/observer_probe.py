#!/usr/bin/env python3
"""Deterministic observer contract probe; it never runs a product workload."""
import argparse
import ctypes
import errno
import gzip
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import traceback

SYSCALLS = ('fsync', 'fdatasync', 'rename', 'renameat', 'renameat2',
            'mkdir', 'mkdirat', 'unlink', 'unlinkat')
STATUS_FIELDS = ('NoNewPrivs', 'Seccomp', 'Seccomp_filters')


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def proc_status(path='/proc/self/status'):
    raw = Path(path).read_text()
    values = {}
    for line in raw.splitlines():
        key, separator, value = line.partition(':')
        if separator and key in STATUS_FIELDS:
            values[key] = int(value.strip())
    return dict(values=values, raw=raw)


def syscall_worker(root, role):
    require(sys.platform == 'linux' and os.uname().machine == 'x86_64',
            'deterministic fixture requires Linux x86_64 syscall ABI')
    libc = ctypes.CDLL(None, use_errno=True)
    # Linux x86_64 syscall ABI, not libc aliases (renameat2(flags=0) may call renameat).
    numbers = dict(fsync=74, fdatasync=75, rename=82, renameat=264, renameat2=316,
                   mkdir=83, mkdirat=258, unlink=87, unlinkat=263)
    signatures = dict(fsync=[ctypes.c_int], fdatasync=[ctypes.c_int],
                      rename=[ctypes.c_char_p, ctypes.c_char_p],
                      renameat=[ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p],
                      renameat2=[ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint],
                      mkdir=[ctypes.c_char_p, ctypes.c_uint],
                      mkdirat=[ctypes.c_int, ctypes.c_char_p, ctypes.c_uint],
                      unlink=[ctypes.c_char_p], unlinkat=[ctypes.c_int, ctypes.c_char_p, ctypes.c_int])
    libc.syscall.restype = ctypes.c_long
    record = dict(role=role, pid=os.getpid(), tid=threading.get_native_id(),
                  proc_self=proc_status(), proc_thread_self=proc_status('/proc/thread-self/status'), calls=[])
    directory = os.open(root, os.O_RDONLY | os.O_DIRECTORY)

    def path(name):
        return os.fsencode(root / f'{role}-{name}')

    def relative(name):
        return os.fsencode(f'{role}-{name}')

    def invoke(name, arguments, expected_errno=0):
        ctypes.set_errno(0)
        typed_arguments = [kind(value) for kind, value in zip(signatures[name], arguments)]
        result = libc.syscall(ctypes.c_long(numbers[name]), *typed_arguments)
        actual_errno = ctypes.get_errno()
        record['calls'].append(dict(syscall=name, result=result, errno=actual_errno,
                                    expected_errno=expected_errno))
        require((result, actual_errno) == ((-1, expected_errno) if expected_errno else (0, 0)),
                f'{role} {name}: unexpected result/errno {(result, actual_errno)}')

    try:
        descriptor = os.open(path('sync'), os.O_CREAT | os.O_EXCL | os.O_RDWR, 0o600)
        try:
            for name in ('fsync', 'fdatasync'):
                invoke(name, (descriptor,))
                invoke(name, (-1,), errno.EBADF)
        finally:
            os.close(descriptor)
        for name in ('rename', 'renameat', 'renameat2'):
            descriptor = os.open(path(name + '-source'), os.O_CREAT | os.O_EXCL | os.O_RDWR, 0o600)
            os.close(descriptor)
            if name == 'rename':
                arguments = (path(name + '-source'), path(name + '-target'))
            else:
                arguments = (directory, relative(name + '-source'), directory, relative(name + '-target'))
                if name == 'renameat2':
                    arguments += (0,)
            invoke(name, arguments)
            invoke(name, arguments, errno.ENOENT)
        for name in ('mkdir', 'mkdirat'):
            arguments = (path(name), 0o700) if name == 'mkdir' else (directory, relative(name), 0o700)
            invoke(name, arguments)
            invoke(name, arguments, errno.EEXIST)
        for name in ('unlink', 'unlinkat'):
            descriptor = os.open(path(name), os.O_CREAT | os.O_EXCL | os.O_RDWR, 0o600)
            os.close(descriptor)
            arguments = (path(name),) if name == 'unlink' else (directory, relative(name), 0)
            invoke(name, arguments)
            invoke(name, arguments, errno.ENOENT)
    finally:
        os.close(directory)
    return record


def worker_main(root):
    # The driver created root before tracing; this process creates no fixture parent directories.
    results = [syscall_worker(root, 'parent')]
    reader, writer = os.pipe()
    child = os.fork()
    if child == 0:
        os.close(reader)
        try:
            payload = dict(status='completed', worker=syscall_worker(root, 'fork-child'))
            code = 0
        except BaseException as error:
            payload, code = dict(status='failed', error=repr(error)), 1
        os.write(writer, json.dumps(payload).encode())
        os.close(writer)
        os._exit(code)
    os.close(writer)
    try:
        chunks = []
        while chunk := os.read(reader, 65536):
            chunks.append(chunk)
    finally:
        os.close(reader)
    _, status = os.waitpid(child, 0)
    payload = json.loads(b''.join(chunks))
    require(os.waitstatus_to_exitcode(status) == 0 and payload['status'] == 'completed',
            f'fork worker failed: {payload}')
    results.append(payload['worker'])
    thread_results = []

    def threaded():
        try:
            thread_results.append(dict(status='completed', worker=syscall_worker(root, 'thread')))
        except BaseException as error:
            thread_results.append(dict(status='failed', error=repr(error)))

    thread = threading.Thread(target=threaded, daemon=True)
    thread.start()
    thread.join(timeout=10)
    require(not thread.is_alive() and len(thread_results) == 1 and
            thread_results[0]['status'] == 'completed', f'thread worker failed: {thread_results}')
    results.append(thread_results[0]['worker'])
    print(json.dumps(dict(status='completed', scope='deterministic-syscall-fixture-only', workers=results)), flush=True)


def parse_counts(text):
    counts, total = {}, None
    for line in text.splitlines():
        fields = line.split()
        if fields and fields[-1] in (*SYSCALLS, 'total'):
            require(len(fields) in (5, 6), f'unrecognized strace count row: {line}')
            row = dict(calls=int(fields[3]), errors=int(fields[4]) if len(fields) == 6 else 0)
            if fields[-1] == 'total':
                require(total is None, 'duplicate strace total')
                total = row
            else:
                require(fields[-1] not in counts, 'duplicate strace syscall row')
                counts[fields[-1]] = row
    require(counts == {name: dict(calls=6, errors=3) for name in SYSCALLS},
            f'expected each syscall calls=6/errors=3, observed {counts}')
    require(total == dict(calls=54, errors=27), f'expected total calls=54/errors=27, observed {total}')
    return dict(syscalls=counts, total=total)


def load_runtime():
    source = Path(__file__).with_name('run.py')
    spec = importlib.util.spec_from_file_location('namespace_observer_runtime', source)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    require(tuple(module.TRACE_SYSCALLS) == SYSCALLS, 'observer syscall filter differs from fixture')
    return module


def bounded_capture(command, output, deadline, runtime, timeout=60):
    record = dict(command=command, timeout_seconds=timeout, started_unix_ns=time.time_ns())
    output.with_suffix('.command.json').write_text(json.dumps(record, indent=2) + '\n')
    child = None
    try:
        with output.with_suffix('.stdout').open('wb') as stdout, output.with_suffix('.stderr').open('wb') as stderr:
            remaining = min(timeout, deadline - time.time())
            require(remaining > 0, 'observer probe overall deadline exhausted')
            child = subprocess.Popen(command, stdout=stdout, stderr=stderr, start_new_session=True,
                                     preexec_fn=runtime.limits)
            record['pid'] = child.pid
            try:
                record['returncode'] = child.wait(timeout=remaining)
                record['timed_out'] = False
            except subprocess.TimeoutExpired:
                runtime.stop_process_group(child)
                record.update(returncode=child.wait(), timed_out=True)
    finally:
        if child is not None and child.poll() is None:
            runtime.stop_process_group(child)
        record['finished_unix_ns'] = time.time_ns()
        output.with_suffix('.command.json').write_text(json.dumps(record, indent=2) + '\n')
    return record


def driver_main(output, deadline):
    output.mkdir(parents=True, exist_ok=False)
    summary = dict(schema='namespace-observer-probe-v1', status='incomplete', mode='observer-validation',
                   scope='deterministic-9-syscall-contract-only', deadline_unix=deadline,
                   probes=[dict(mode=mode, status='not_run', reason='probe_not_started')
                           for mode in ('plain', 'seccomp-bpf')],
                   github_run_id=os.environ.get('GITHUB_RUN_ID'),
                   github_run_attempt=os.environ.get('GITHUB_RUN_ATTEMPT'),
                   carrier_sha=os.environ.get('GITHUB_SHA'), github_job=os.environ.get('GITHUB_JOB'),
                   runner_name=os.environ.get('RUNNER_NAME'),
                   probe_script_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest())
    code = 2
    try:
        runtime = load_runtime()
        require(all(summary[key] for key in ('github_run_id', 'github_run_attempt', 'carrier_sha',
                                             'github_job', 'runner_name')), 'GitHub execution identity is incomplete')
        summary['ambient'] = proc_status()
        ambient = summary['ambient']['values']
        require(set(ambient) == set(STATUS_FIELDS) and ambient['Seccomp'] == 0 and
                ambient['Seccomp_filters'] == 0, 'ambient seccomp admission is unavailable or filtered')
        program = shutil.which('strace')
        require(program is not None, 'strace executable is unavailable')
        strace = Path(program).resolve()
        summary['strace'] = dict(path=str(strace), sha256=hashlib.sha256(strace.read_bytes()).hexdigest())
        for label, option in (('version', '--version'), ('help', '--help')):
            result = bounded_capture([str(strace), option], output / label, deadline, runtime, timeout=10)
            require(result['returncode'] == 0 and not result['timed_out'], f'strace {option} unavailable')
        version_bytes = (output / 'version.stdout').read_bytes()
        summary['strace'].update(version=version_bytes.decode(),
                                  version_sha256=hashlib.sha256(version_bytes).hexdigest())
        with gzip.open('/usr/share/man/man1/strace.1.gz', 'rb') as source:
            (output / 'strace.man').write_bytes(source.read())
        for record in summary['probes']:
            mode = record['mode']
            # Cleanup is outside the traced process; both probes use independent fresh roots.
            with tempfile.TemporaryDirectory(prefix='owned-observer-probe-', dir=output.parent) as temporary:
                command = runtime.trace_command([sys.executable, '-B', str(Path(__file__).resolve()),
                                                 '--worker', temporary], output, mode, mode)
                record.update(status='incomplete', reason='probe_not_completed')
                record['process'] = bounded_capture(command, output / mode, deadline, runtime)
                record['observer'] = runtime.observer_evidence(output / f'{mode}.stderr', mode, command)
                require(record['process']['returncode'] == 0 and not record['process']['timed_out'],
                        f'{mode}: worker did not complete')
                payload = json.loads((output / f'{mode}.stdout').read_text())
                require(payload['status'] == 'completed' and
                        [row['role'] for row in payload['workers']] == ['parent', 'fork-child', 'thread'],
                        f'{mode}: complete worker evidence missing')
                record['tracees'] = payload
                for worker in payload['workers']:
                    expected = 0 if mode == 'plain' else 2
                    filters = 0 if mode == 'plain' else 1
                    for field in ('proc_self', 'proc_thread_self'):
                        values = worker[field]['values']
                        require(set(values) == set(STATUS_FIELDS) and values['Seccomp'] == expected and
                                values['Seccomp_filters'] == filters,
                                f'{mode}: {worker["role"]} {field} seccomp state is not proven: {values}')
                    require(len(worker['calls']) == 18, f'{mode}: worker syscall contract incomplete')
                record['counts'] = parse_counts((output / f'{mode}.strace').read_text())
                if mode == 'seccomp-bpf':
                    observer = record['observer']
                    require(observer['effective_mode'] == mode and
                            observer['activation_status'] == 'enabled_and_observed',
                            'BPF activation or EVENT_SECCOMP(7) evidence is missing/conflicting')
                    require({row['tid'] for row in payload['workers']} <= set(observer['seccomp_event_pids']),
                            'BPF events do not prove tracing parent, fork child and thread')
                record['status'] = 'completed'
                record.pop('reason')
        require(summary['probes'][0]['counts'] == summary['probes'][1]['counts'],
                'plain/BPF syscall count or error contract differs')
        summary['status'], code = 'completed_probe', 0
    except (OSError, KeyError, ValueError, TypeError, RuntimeError, subprocess.SubprocessError) as error:
        summary.update(reason=str(error), error_type=type(error).__name__)
        (output / 'probe.error.txt').write_text(traceback.format_exc())
    finally:
        (output / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
        (output / 'probe.json').write_text(json.dumps(summary, indent=2) + '\n')
        print(json.dumps(summary), flush=True)
    return code


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--deadline-unix', type=int)
    parser.add_argument('--worker', type=Path)
    args = parser.parse_args()
    if args.worker is not None:
        worker_main(args.worker.resolve())
        return 0
    require(args.output is not None and args.deadline_unix is not None,
            'driver requires --output and --deadline-unix')
    return driver_main(args.output.resolve(), args.deadline_unix)


if __name__ == '__main__':
    raise SystemExit(main())
