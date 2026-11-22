"""Admission and outcome gate for the manual, isolated SMB MOVE job."""

import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import stat
import subprocess
import sys

PROFILE = "SMB-MOVE-CI-1"
SUITE = Path("zxs/tests/acceptance")
DRIVER = SUITE / "guest_smb_move.py"
MATERIALS = SUITE / "guest_smb_move_inputs.json"
LIMIT = 16 * 1024
CASES = {"stock", "original"}
RECEIPTS = {"mounts", "init", "setup", "ready", "client", "system",
            "versions", "retirement", "reap"}


def hex_id(value, length):
    return (isinstance(value, str)
            and re.fullmatch(f"[0-9a-f]{{{length}}}", value) is not None
            and value != "0" * length)


def read_regular(path):
    fd = os.open(path, os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as source:
        info = os.fstat(source.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or not 0 < info.st_size <= LIMIT:
            raise ValueError("not a bounded original acceptance record")
        raw = source.read(LIMIT + 1)
        if len(raw) > LIMIT or len(raw) != info.st_size:
            raise ValueError("incomplete acceptance record")
        return raw


def object_once(items):
    data = dict(items)
    if len(data) != len(items):
        raise ValueError("duplicate acceptance evidence key")
    return data


def outcome(raw, anchor):
    report = json.loads(raw, object_pairs_hook=object_once)
    if not isinstance(report, dict) or set(report) != {*anchor, "cases"}:
        raise ValueError("unexpected acceptance evidence")
    if any(report[key] != expected for key, expected in anchor.items()):
        raise ValueError("acceptance evidence belongs to another run")
    runs = report["cases"]
    if not isinstance(runs, dict) or set(runs) != CASES:
        raise ValueError("both independent move cases must have completed")
    for run in runs.values():
        if not isinstance(run, dict) or set(run) != {"pool", "file", *RECEIPTS}:
            raise ValueError("missing positive case evidence")
        if not hex_id(run["pool"], 32) or not hex_id(run["file"], 32):
            raise ValueError("uncaptured original identities")
        if not all(hex_id(run[key], 64) for key in RECEIPTS):
            raise ValueError("incomplete case receipt hashes")
    if runs["stock"]["pool"] == runs["original"]["pool"]:
        raise ValueError("move cases reused one pool")


def main(argv):
    if len(argv) != 6:
        raise SystemExit("SMB MOVE acceptance needs its reviewed source and inputs")
    source, work = map(Path, argv[:2])
    commit, server_sha, helper_sha, material_sha = argv[2:]
    if not (hex_id(commit, 40) and all(hex_id(v, 64) for v in (server_sha, helper_sha, material_sha))):
        raise SystemExit("invalid SMB MOVE approval")
    driver = source / DRIVER
    if not driver.is_file() or driver.is_symlink():
        raise SystemExit("SMB MOVE acceptance did not run: the reviewed guest driver is missing")
    try:
        approved = read_regular(source / MATERIALS)
    except (OSError, ValueError):
        raise SystemExit("SMB MOVE acceptance did not run: the offline inputs manifest is missing or unqualified") from None
    if hashlib.sha256(approved).hexdigest() != material_sha:
        raise SystemExit("SMB MOVE acceptance did not run: offline inputs changed")

    report = work / "result.json"
    if report.exists() or report.is_symlink():
        raise SystemExit("SMB MOVE acceptance result path was occupied before the run")
    anchor = {"profile": PROFILE, "source": commit, "server_sha256": server_sha,
              "helper_sha256": helper_sha, "guest_inputs_sha256": material_sha,
              "challenge": secrets.token_hex(16)}
    request = work / "request.json"
    with request.open("x", encoding="ascii") as stream:
        json.dump(anchor, stream, sort_keys=True)
        stream.write("\n")
    # Only the fixed driver in the approved source gets a fresh private work
    # area. Its raw QEMU/client/TTY output is not sent to the hosted job log.
    finished = subprocess.run(
        [sys.executable, "-I", "-B", str(driver), str(source), str(work)],
        cwd=work, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        env={"PATH": "/usr/bin:/bin", "LC_ALL": "C", "TZ": "UTC",
             "HOME": str(work), "TMPDIR": str(work)}, check=False,
    )
    if finished.returncode != 0:
        why = "SMB MOVE driver did not complete the two acceptance cases"
        try:
            refusal = json.loads(read_regular(work / "failure.json"), object_pairs_hook=object_once)
            stages = {"admission", "offline-inputs", "mounts", "init", "credential", "local-link", "startup",
                      "web-login", "seed-and-commit", "source-history", "signed-move", "retirement", "target-history", "reap"}
            if (set(refusal) == {*anchor, "case", "phase"} and all(refusal[k] == v for k, v in anchor.items())
                    and refusal["case"] in CASES and refusal["phase"] in stages):
                why = f"SMB MOVE case {refusal['case']} did not complete phase {refusal['phase']}"
        except (OSError, ValueError, TypeError, RecursionError):
            pass
        raise SystemExit(why)
    try:
        raw = read_regular(report)
        outcome(raw, anchor)
    except (OSError, ValueError, RecursionError):
        raise SystemExit("SMB MOVE acceptance did not produce both original evidence records") from None
    print(f"SMB MOVE: both isolated cases complete for {commit}; receipt SHA-256 {hashlib.sha256(raw).hexdigest()}")


if __name__ == "__main__":
    main(sys.argv[1:])
