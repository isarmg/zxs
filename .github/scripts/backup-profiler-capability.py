"""Bounded tool capability checks against fixed, non-business Linux programs."""
import argparse
import json
import os
import re
import select
import selectors
import shutil
import signal
import subprocess
import time
from dataclasses import dataclass

LIMIT = 64 * 1024
ENV = {
    "PATH": "/usr/bin:/bin",
    "LANG": "C",
    "LC_ALL": "C",
    "HOME": "/nonexistent",
    "DEBUGINFOD_URLS": "",
    "GDBHISTFILE": "/dev/null",
}
PROGRAMS = (("/usr/bin/true", 0), ("/usr/bin/false", 1))


@dataclass
class Capture:
    status: str
    returncode: object
    output: bytes
    cleanup: str = "not_needed"


def _child_identity(pid):
    with open(f"/proc/{pid}/stat", "rb") as stream:
        raw = stream.read(4097)
    if len(raw) > 4096 or not raw.endswith(b"\n"):
        raise ValueError("unbounded or incomplete process identity")
    fields = raw[raw.rfind(b")") + 2:].split()
    return int(fields[1]), int(fields[19])


def _inferior_handle(output, parent, program):
    match = re.search(rb'=thread-group-started,id="i1",pid="([0-9]+)"', output)
    if match is None:
        return None
    pid = int(match.group(1))
    if pid <= 1 or pid == os.getpid():
        return None
    handle = None
    try:
        before = _child_identity(pid)
        if before[0] != parent:
            return None
        handle = os.pidfd_open(pid)
        executable = os.stat(f"/proc/{pid}/exe")
        expected = os.stat(program)
        if (
            _child_identity(pid) != before
            or (executable.st_dev, executable.st_ino) != (expected.st_dev, expected.st_ino)
        ):
            os.close(handle)
            return None
        return handle
    except (OSError, ValueError, IndexError):
        if handle is not None:
            os.close(handle)
        return None


def _cleanup(process, inferior):
    # Only our direct child and an identity-bound, explicitly reported inferior.
    # No process scan, attach, process-group kill, or escaped-descendant claim.
    confirmed = True
    if inferior is not None:
        try:
            signal.pidfd_send_signal(inferior, signal.SIGKILL)
        except ProcessLookupError:
            pass
        except OSError:
            confirmed = False
        try:
            confirmed = bool(select.select([inferior], [], [], 1)[0]) and confirmed
        except OSError:
            confirmed = False
    if process.poll() is None:
        try:
            process.kill()
        except ProcessLookupError:
            pass
        except OSError:
            confirmed = False
    try:
        if process.wait(timeout=1) is None:
            confirmed = False
    except (OSError, subprocess.TimeoutExpired):
        confirmed = False
    return "confirmed" if confirmed else "unconfirmed"


def _execute(arguments, seconds, *, program=None, overall=None):
    deadline = time.monotonic() + seconds
    if overall is not None:
        deadline = min(deadline, overall)
    if time.monotonic() >= deadline:
        return Capture("not_run_deadline", None, b"", "not_started")
    try:
        process = subprocess.Popen(
            arguments,
            cwd="/",
            env=ENV,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
    except OSError:
        return Capture("spawn_error", None, b"", "not_started")
    output = bytearray()
    inferior = None
    status = "internal_error"
    cleanup = "not_needed"
    selector = None
    try:
        selector = selectors.DefaultSelector()
        os.set_blocking(process.stdout.fileno(), False)
        selector.register(process.stdout, selectors.EVENT_READ)
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                status = "timeout"
                break
            events = selector.select(min(remaining, 0.05))
            for key, _ in events:
                chunk = os.read(key.fileobj.fileno(), min(4096, LIMIT + 1 - len(output)))
                if not chunk:
                    selector.unregister(key.fileobj)
                    continue
                output.extend(chunk)
                if len(output) > LIMIT:
                    status = "output_limit"
                    break
                if program is not None and inferior is None:
                    inferior = _inferior_handle(bytes(output), process.pid, program)
            if status == "output_limit":
                break
            if time.monotonic() >= deadline:
                status = "timeout"
                break
            if process.poll() is not None and not selector.get_map():
                status = "completed"
                break
    except Exception:
        status = "internal_error"
    finally:
        if status == "completed" and inferior is not None:
            try:
                if not select.select([inferior], [], [], 0)[0]:
                    status = "inferior_still_running"
            except OSError:
                status = "inferior_exit_unconfirmed"
        if status != "completed":
            cleanup = _cleanup(process, inferior)
            if (
                program is not None
                and inferior is None
            ):
                cleanup = "inferior_unconfirmed"
        if inferior is not None:
            os.close(inferior)
        if selector is not None:
            selector.close()
        process.stdout.close()
    return Capture(status, process.returncode, bytes(output[:LIMIT]), cleanup)


def _category(output):
    lowered = output.lower()
    if any(word in lowered for word in (
        b"permission", b"operation not permitted", b"access denied",
    )):
        return "permission_denied"
    if any(word in lowered for word in (
        b"not supported", b"unknown command", b"unrecognized option", b"undefined command",
    )):
        return "unsupported"
    return "tool_error"


def _version(tool, executable, overall):
    result = _execute([executable, "--version"], 2, overall=overall)
    pattern = (
        rb"perf version ([0-9]+(?:\.[0-9]+){0,3})"
        if tool == "perf"
        else rb"GNU gdb[^\n]*? ([0-9]+(?:\.[0-9]+){1,3})"
    )
    found = re.search(pattern, result.output)
    return {
        "status": result.status,
        "returncode": result.returncode,
        "number": found.group(1).decode("ascii") if found else None,
        "cleanup": result.cleanup,
    }


def _gdb_arguments(executable, program):
    arguments = [
        executable, "--nx", "--nh", "--quiet", "--batch",
        "--return-child-result", "--interpreter=mi2",
    ]
    for command in (
        "set auto-load off",
        "set auto-load safe-path /dev/null",
        "set debuginfod enabled off",
        "set startup-with-shell off",
        "set disable-randomization off",
        "set print pretty off",
    ):
        arguments.extend(["-iex", command])
    arguments.extend(["-ex", "run", "--args", program])
    return arguments


def _perf_counted(output):
    for line in output.splitlines():
        fields = line.split(b";")
        if (
            len(fields) >= 3
            and fields[2].strip() == b"cpu-clock:u"
            and re.fullmatch(rb"[0-9]+(?:\.[0-9]+)?", fields[0].strip())
        ):
            return True
    return False


def _gdb_exited(output, expected):
    if re.search(rb"(?m)^\^error(?:,|$)", output):
        return False
    if expected == 0:
        return b'*stopped,reason="exited-normally"' in output
    return re.search(rb'\*stopped,reason="exited",exit-code="0*1"', output) is not None


def _probe(tool, executable, program, expected, overall):
    if tool == "gdb":
        arguments = _gdb_arguments(executable, program)
        result = _execute(arguments, 5, program=program, overall=overall)
        evidence = _gdb_exited(result.output, expected)
    else:
        arguments = [
            executable, "stat", "--no-big-num", "-x", ";",
            "-e", "cpu-clock:u", "--", program,
        ]
        result = _execute(arguments, 5, program=program, overall=overall)
        evidence = _perf_counted(result.output)
    status = result.status
    if status == "completed":
        status = (
            "available"
            if result.returncode == expected and evidence
            and result.cleanup == "not_needed"
            and _category(result.output) == "tool_error"
            else _category(result.output)
        )
    return {"status": status, "returncode": result.returncode, "cleanup": result.cleanup}


def _execution_failed(observation):
    return (
        observation["status"] in {
            "spawn_error", "timeout", "output_limit", "internal_error",
            "not_run_deadline", "inferior_still_running", "inferior_exit_unconfirmed",
        }
        or observation.get("cleanup") in {"unconfirmed", "inferior_unconfirmed"}
    )


def report(head):
    overall = time.monotonic() + 40
    result = {
        "format": 1,
        "head": head,
        "scope": "fixed_non_business_children",
        "status": "failed",
        "complete": False,
        "tools": {
            tool: {"present": None, "status": "not_run", "probes": []}
            for tool in ("perf", "gdb")
        },
    }
    for tool, entry in result["tools"].items():
        if time.monotonic() >= overall:
            entry["status"] = "not_run_deadline"
            return result
        executable = shutil.which(tool, path=ENV["PATH"])
        entry["present"] = executable is not None
        if executable is None:
            entry["status"] = "not_installed"
            continue
        entry["version"] = _version(tool, executable, overall)
        if _execution_failed(entry["version"]):
            entry["status"] = "execution_failed"
            return result
        if tool == "gdb" and not (
            hasattr(os, "pidfd_open") and hasattr(signal, "pidfd_send_signal")
        ):
            entry["status"] = "cleanup_unsupported"
            continue
        if not all(os.path.isfile(program) for program, _ in PROGRAMS):
            entry["status"] = "probe_program_unavailable"
            continue
        for program, expected in PROGRAMS:
            observation = _probe(tool, executable, program, expected, overall)
            observation["expected_returncode"] = expected
            entry["probes"].append(observation)
            if _execution_failed(observation):
                entry["status"] = "execution_failed"
                return result
        entry["status"] = (
            "available"
            if all(item["status"] == "available" for item in entry["probes"])
            else "unavailable"
        )
    result["status"] = "completed"
    result["complete"] = True
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True)
    parser.add_argument("--head", required=True)
    arguments = parser.parse_args()
    if re.fullmatch(r"[0-9a-f]{40}", arguments.head) is None:
        print('{"status":"invalid_head"}')
        return 2
    try:
        result = report(arguments.head)
    except KeyboardInterrupt:
        print('{"status":"interrupted"}')
        return 130
    except Exception:
        print('{"status":"diagnostic_failed"}')
        return 1
    encoded = json.dumps(result, sort_keys=True).encode("ascii") + b"\n"
    try:
        descriptor = os.open(
            arguments.output,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
            0o600,
        )
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(encoded)
    except OSError:
        print('{"status":"report_write_failed"}')
        return 1
    print(encoded.decode("ascii"), end="")
    # Unavailable tools are a completed diagnostic, not a project test result.
    # Execution/cleanup failure is fail-closed even though the report is retained.
    return 0 if result["complete"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
