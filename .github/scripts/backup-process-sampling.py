#!/usr/bin/env python3
"""Bounded, best-effort Linux observations of one directly launched test process.

The caller retains Cargo artifact/SHA admission and exact test selection. Diagnostic
records are not test outcomes. No debugger, perf, process search or product hook.
"""
import json
import os
import re
import shutil
from pathlib import Path
import subprocess
import time

INTERVAL = 5.0
MAX_SAMPLES = 180
MAX_THREADS = 64
READ_LIMIT = 8192


def read_text(path):
    try:
        with open(path, 'rb') as stream:
            data = stream.read(READ_LIMIT + 1)
        if len(data) > READ_LIMIT:
            return None, 'truncated'
        if not data or not data.endswith(b'\n'):
            return None, 'short'
        return data.decode('ascii'), 'ok'
    except FileNotFoundError:
        return None, 'gone'
    except PermissionError:
        return None, 'denied'
    except (OSError, UnicodeError):
        return None, 'unavailable'


def parse_stat(text):
    # comm may contain whitespace and parentheses; never retain it.
    first, last = text.find('('), text.rfind(')')
    if first < 1 or last <= first:
        raise ValueError('stat framing')
    pid = int(text[:first].strip())
    fields = text[last + 1:].split()
    if len(fields) < 22 or fields[0] not in set('RSDZTtXxKWPI'):
        raise ValueError('stat fields')
    numbers = [int(fields[i]) for i in (11, 12, 17, 19, 21)]
    if any(value < 0 for value in numbers):
        raise ValueError('negative stat field')
    return dict(pid=pid, state=fields[0], user_ticks=numbers[0],
                system_ticks=numbers[1], threads=numbers[2],
                start_ticks=numbers[3], rss_pages=numbers[4])


def stat_record(path):
    text, status = read_text(path)
    if status != 'ok':
        return None, status
    try:
        return parse_stat(text), 'ok'
    except (ValueError, IndexError):
        return None, 'malformed'


def optional_record(path, kind):
    text, status = read_text(path)
    if status != 'ok':
        return dict(status=status)
    try:
        if kind == 'io':
            allowed = {'rchar', 'wchar', 'syscr', 'syscw', 'read_bytes', 'write_bytes'}
            values = {}
            for line in text.splitlines():
                key, value = line.split(':', 1)
                if key in allowed:
                    values[key] = int(value)
            if set(values) != allowed or any(value < 0 for value in values.values()):
                raise ValueError('io fields')
        else:
            values_list = [int(value) for value in text.split()]
            if len(values_list) != 3 or any(value < 0 for value in values_list):
                raise ValueError('schedstat fields')
            values = dict(zip(('cpu_ns', 'wait_ns', 'slices'), values_list))
        return dict(status='ok', **values)
    except (ValueError, TypeError):
        return dict(status='malformed')


def wchan_record(path):
    # Unlike stat/io, proc wchan commonly has no trailing newline.
    try:
        with open(path, 'rb') as stream:
            data = stream.read(129)
        if len(data) > 128:
            return dict(status='truncated')
        symbol = data.decode('ascii').removesuffix('\n')
        if not symbol:
            return dict(status='short')
        if not re.fullmatch(r'(?:0|[A-Za-z_][A-Za-z0-9_.$]*)', symbol):
            return dict(status='malformed')
        return dict(status='ok', symbol=symbol)
    except FileNotFoundError:
        return dict(status='gone')
    except PermissionError:
        return dict(status='denied')
    except (OSError, UnicodeError):
        return dict(status='unavailable')


def perf_availability():
    # Presence/policy evidence only, NOT proof that perf can sample this runner.
    row = dict(tool_present=None, paranoid=None, status='unavailable')
    try:
        row['tool_present'] = shutil.which('perf') is not None
        text, status = read_text('/proc/sys/kernel/perf_event_paranoid')
        row['status'] = status
        if status == 'ok':
            row['paranoid'] = int(text.strip())
    except (ValueError, TypeError):
        row['status'] = 'malformed'
    except Exception:
        row['status'] = 'unavailable'
    return row


class Observer:
    def __init__(self, pid, executable, destination):
        self.pid = pid
        self.executable = executable
        self.destination = destination
        self.identity = None
        self.samples = 0
        self.started = time.monotonic()
        self.perf = perf_availability()
        try:
            self.clock_ticks = os.sysconf('SC_CLK_TCK')
            if self.clock_ticks <= 0:
                raise ValueError('invalid clock rate')
            self.clock_status = 'ok'
        except Exception:
            self.clock_ticks = None
            self.clock_status = 'unavailable'

    def emit(self, value):
        # Diagnostic failures must not change the child's result or exception.
        try:
            with open(self.destination, 'a', encoding='utf-8') as stream:
                stream.write(json.dumps(value, separators=(',', ':')) + '\n')
        except Exception:
            pass

    def sample(self):
        if self.samples >= MAX_SAMPLES:
            return
        self.samples += 1
        row = dict(sample=self.samples, status='unavailable', process=None,
                   io=None, threads=[], threads_truncated=False,
                   elapsed_seconds=None, clock_ticks=self.clock_ticks,
                   clock_status=self.clock_status, perf_availability=self.perf)
        try:
            row['elapsed_seconds'] = max(0.0, time.monotonic() - self.started)
            root = Path('/proc') / str(self.pid)
            process, status = stat_record(root / 'stat')
            if status != 'ok':
                row['status'] = status
                self.emit(row)
                return
            actual = os.stat(root / 'exe')
            expected = os.stat(self.executable)
            identity = (process['pid'], process['start_ticks'],
                        actual.st_dev, actual.st_ino)
            if (process['pid'] != self.pid
                    or (actual.st_dev, actual.st_ino) != (expected.st_dev, expected.st_ino)
                    or (self.identity is not None and self.identity != identity)):
                row['status'] = 'identity-mismatch'
                self.emit(row)
                return
            self.identity = identity
            row.update(status='ok', process=process,
                       io=optional_record(root / 'io', 'io'))
            # Bound enumeration as well as file reads. No scan of other processes.
            with os.scandir(root / 'task') as entries:
                for entry in entries:
                    if not entry.name.isdecimal():
                        continue
                    if len(row['threads']) == MAX_THREADS:
                        row['threads_truncated'] = True
                        break
                    tid = int(entry.name)
                    thread_root = root / 'task' / entry.name
                    thread, thread_status = stat_record(thread_root / 'stat')
                    if thread is not None and thread['pid'] != tid:
                        thread, thread_status = None, 'identity-mismatch'
                    thread_row = dict(tid=tid, status=thread_status, stat=thread,
                                      schedstat=None, wchan=None)
                    if thread_status == 'ok':
                        thread_row['schedstat'] = optional_record(
                            thread_root / 'schedstat', 'schedstat')
                        thread_row['wchan'] = wchan_record(thread_root / 'wchan')
                        after_thread, after_status = stat_record(thread_root / 'stat')
                        if after_status != 'ok':
                            thread_row['status'] = after_status
                        elif (after_thread['pid'], after_thread['start_ticks']) != (
                                thread['pid'], thread['start_ticks']):
                            thread_row['status'] = 'changed-during-sample'
                    if thread_row['status'] != 'ok':
                        thread_row.update(stat=None, schedstat=None, wchan=None)
                    row['threads'].append(thread_row)
            after, status = stat_record(root / 'stat')
            if status != 'ok':
                row['status'] = status
            else:
                after_exe = os.stat(root / 'exe')
                after_identity = (after['pid'], after['start_ticks'],
                                  after_exe.st_dev, after_exe.st_ino)
                if after_identity != identity:
                    row['status'] = 'changed-during-sample'
        except FileNotFoundError:
            row['status'] = 'gone'
        except PermissionError:
            row['status'] = 'denied'
        except Exception:
            row['status'] = 'unavailable'
        if row['status'] != 'ok':
            row.update(process=None, io=None, threads=[], threads_truncated=False)
        self.emit(row)


def run(args, *, cwd, env, stdout, stderr, timeout, diagnostic_path):
    """run's redirected-output subset, with the original single wall deadline.

    Caller must supply the already SHA-verified absolute executable as args[0].
    No capture_output/check/shell support is added. Timeout cleanup intentionally
    matches the direct-child scope of subprocess.run, not descendant admission.
    """
    process = subprocess.Popen(args, cwd=cwd, env=env, stdout=stdout, stderr=stderr)
    try:
        deadline = time.monotonic() + timeout
        observer = None
        try:
            observer = Observer(process.pid, args[0], diagnostic_path)
        except Exception:
            pass
        next_sample = time.monotonic()
        while True:
            now = time.monotonic()
            if now >= deadline:
                # Diagnostics cannot prove a terminal status preceded this deadline.
                raise subprocess.TimeoutExpired(args, timeout)
            if observer is not None and now >= next_sample:
                try:
                    observer.sample()
                except Exception:
                    # Includes unexpected parsing/logging errors in diagnostics only.
                    pass
                next_sample = time.monotonic() + INTERVAL
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                continue
            wait = (remaining if observer is None else
                    min(remaining, max(0.001, next_sample - time.monotonic())))
            try:
                returncode = process.wait(timeout=wait)
            except subprocess.TimeoutExpired:
                continue
            if time.monotonic() >= deadline:
                raise subprocess.TimeoutExpired(args, timeout)
            return subprocess.CompletedProcess(args, returncode)
    except BaseException:
        process.kill()
        process.wait()
        raise
