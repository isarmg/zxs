"""Prepare and check a disposable fixture for the installed Linux Qt workflow."""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import zipfile

CLI = "/usr/bin/zip-workspace"
DESKTOP = "/usr/bin/zip-workspace-desktop"
LIMIT = 64 * 1024
LEAF = "design"
DIRS = {"A", "B", "empty", "missing"}
SEED = {
    "A/report.txt": b"A\nbefore\n",
    "B/report.txt": b"B\nunchanged\n",
    "missing/restore.txt": b"restore this exact text\n",
}
EDITED = {**SEED, "A/report.txt": b"A\nafter!\n", "A/local.txt": b"new local line\n"}
WORK = "work"
STATE = "states"
STORE = "store"
RECORD = "seed.json"


def require(ok, message):
    if not ok:
        raise ValueError(message)


def run_json(*args):
    command = [CLI, *map(str, args)]
    result = subprocess.run(command, check=True, stdout=subprocess.PIPE, text=True, encoding="utf-8")
    return json.loads(result.stdout)


def private_directory(path):
    info = path.lstat()
    require(stat.S_ISDIR(info.st_mode) and info.st_uid == os.getuid() and stat.S_IMODE(info.st_mode) == 0o700,
            f"expected an existing, user-owned 0700 directory: {path}")


def fixture_location(spelling, *, prepare):
    require(sys.platform == "linux" and os.getuid() == os.geteuid() != 0,
            "use an ordinary account in the isolated Linux guest")
    require(os.path.isfile(CLI) and os.path.isfile(DESKTOP),
            "install the matching CLI and Qt desktop candidate packages")
    root = Path(spelling)
    require(root.anchor == "/" and root.name and str(root) == spelling and ".." not in root.parts,
            "give one literal absolute run directory")
    require(not root.is_relative_to(Path(__file__).resolve().parents[2]),
            "keep the fixture outside the source tree")
    require(root.parent.resolve(strict=True) == root.parent, "scratch must not use symlink ancestors")
    for parent in (root.parent, *root.parent.parents):
        info = parent.stat()
        require(stat.S_ISDIR(info.st_mode) and info.st_uid in (0, os.getuid()) and not info.st_mode & 0o022,
                f"untrusted scratch ancestor: {parent}")
    private_directory(root.parent)
    if not prepare:
        private_directory(root)
    probe = root.parent if prepare else root
    found = subprocess.check_output(
        ["/usr/bin/findmnt", "--kernel", "--target", str(probe), "--noheadings", "--raw",
         "--output", "FSTYPE,FSROOT"], text=True).split()
    require(found in (["ext4", "/"], ["xfs", "/"]), "use a whole local ext4/XFS mount")
    return root


def small_regular(path):
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC)
    with os.fdopen(fd, "rb") as stream:
        require(stat.S_ISREG(os.fstat(fd).st_mode), f"not an ordinary fixture file: {path}")
        data = stream.read(LIMIT + 1)
    require(len(data) <= LIMIT, f"not a small fixture file: {path}")
    return data


def archive_leaf(marker, generation):
    for field in ("repo", "branch", "operation"):
        require(re.fullmatch("[0-9a-f]{32}", marker[field]) is not None, f"invalid fixture {field}")
    return f"{marker['repo']}.{marker['branch']}.{generation:016x}.zxs"


def check_archive(path, files):
    data = small_regular(path)
    marker = run_json("inspect", path)["commit"]
    require(path.name == archive_leaf(marker, marker["generation"]), "not the canonical snapshot name")
    expected = {**{name + "/": b"" for name in DIRS}, **files}
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        members = archive.infolist()
        require(len(members) == len(expected) and {item.filename for item in members} == set(expected),
                f"archive path or empty-directory mismatch: {path}")
        for item in members:
            want = expected[item.filename]
            require(item.compress_type == zipfile.ZIP_STORED and item.file_size == len(want)
                    and archive.read(item) == want, f"wrong Store member bytes: {item.filename}")
    return marker, hashlib.sha256(data).hexdigest()


def prepare(root):
    root.mkdir(mode=0o700)  # Named by the operator; exclusive, never adopted or removed.
    print(f"Fixture directory: {root}", file=sys.stderr, flush=True)
    seed = root / "seed"
    for path in (seed, root / WORK, root / STATE, root / STORE):
        path.mkdir(mode=0o700)
    for name in sorted(DIRS):
        (seed / name).mkdir(mode=0o700)
    for name, body in SEED.items():
        with (seed / name).open("xb") as stream:
            stream.write(body)
    state, store = root / STATE / "seed", root / STORE
    run_json("init", seed, store, state)
    receipt = run_json("commit", state, "--writers-stopped")
    archive = store / archive_leaf(receipt["commit"], 1)
    require(receipt["committed_archive"] == str(archive), "seed published outside the expected slot")
    marker, sha = check_archive(archive, SEED)
    require(marker == receipt["commit"] and marker["generation"] == 1 and marker["parent"] is None,
            "seed must be the first genuine repository node")
    record = {"seed": marker, "sha256": sha}
    with (root / RECORD).open("x", encoding="ascii") as stream:
        json.dump(record, stream)
    return {"run": str(root), "archive": str(archive), **record}


def check_tree(work, files):
    require(stat.S_ISDIR(work.lstat().st_mode), "expected the original visible fixture directory")
    for folder in ("", *sorted(DIRS)):
        base = work / folder
        require(stat.S_ISDIR(base.lstat().st_mode), f"expected a real directory: {base}")
        wanted = DIRS if not folder else {Path(name).name for name in files if Path(name).parent == Path(folder)}
        require({entry.name for entry in base.iterdir()} == wanted, f"unexpected entries in {base}")
    for name, body in files.items():
        require(small_regular(work / name) == body, f"worktree byte mismatch: {name}")


def verify(root, phase, checkout_id, commit_id):
    store, work, state = root / STORE, root / WORK / LEAF, root / STATE / LEAF
    for directory in (store, work.parent, state.parent, state):
        private_directory(directory)
    recorded = json.loads(small_regular(root / RECORD))
    seed, seed_sha = recorded["seed"], recorded["sha256"]
    base = store / archive_leaf(seed, 1)
    require(check_archive(base, SEED) == (seed, seed_sha), "the original published snapshot changed")
    checkout = run_json("checkout-status", state)
    status = run_json("status", state)
    require(checkout["operation"] == checkout_id and checkout["worktree"] == status["worktree"]
            and checkout["source_node"] == seed["operation"] and checkout["recorded_stage"] == "attached"
            and checkout["chosen_destination"] == str(work), "checkout receipt differs from the GUI operation")
    require(hashlib.sha256(small_regular(state / "original.zxs")).hexdigest() == seed_sha,
            "the private checkout copy differs from the original snapshot")
    final = phase == "committed"
    expected = SEED if phase == "checked-out" else EDITED
    if phase == "edited":
        expected = {name: body for name, body in expected.items() if name != "missing/restore.txt"}
    check_tree(work, expected)
    require(status["visibility"] == "visible" and status["pending_visibility_operation"] is None
            and status["pending_operation"] is None and status["repo"] == seed["repo"]
            and status["branch"] == seed["branch"] and status["base_generation"] == (2 if final else 1)
            and status["base_operation"] == (commit_id if final else seed["operation"]),
            "registered base or pending state is not the expected one")
    deltas = {
        "added": ["A/local.txt"] if phase in ("edited", "restored") else [],
        "changed": ["A/report.txt"] if phase in ("edited", "restored") else [],
        "removed": ["missing/restore.txt"] if phase == "edited" else [],
    }
    changes = run_json("changes", state)
    require(changes == {kind: {"count": len(paths), "first_paths": paths} for kind, paths in deltas.items()},
            "CLI changes differ from the expected GUI report")
    slots = {base.name}
    output = {"phase": phase, "archive": str(base), "archive_sha256": seed_sha,
              "checkout": checkout, "state": status, "changes": changes}
    if final:
        path = store / archive_leaf(seed, 2)
        marker, digest = check_archive(path, EDITED)
        require((marker["repo"], marker["branch"], marker["generation"], marker["operation"], marker["parent"])
                == (seed["repo"], seed["branch"], 2, commit_id, seed["operation"]),
                "published child does not match the exact GUI commit and its parent")
        require(run_json("history", path, store) == {"commits": [marker, seed], "missing_parent": None},
                "missing original parent or unexpected history")
        candidate = small_regular(state / (commit_id + ".zxs"))
        require(hashlib.sha256(candidate).hexdigest() == digest, "new archive does not match the private candidate")
        slots.add(path.name)
        output.update(new_archive=str(path), new_archive_sha256=digest, new_commit=marker)
    require({p.name for p in store.iterdir()} == slots, "unexpected STORE entry or upload remnant")
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    start = commands.add_parser("prepare", help="register and commit only the new synthetic seed")
    start.add_argument("run")
    check = commands.add_parser("verify", help="read fixture bytes, CLI status and receipts; do not resume")
    check.add_argument("run")
    check.add_argument("phase", choices=("checked-out", "edited", "restored", "committed"))
    check.add_argument("checkout_id", help="original operation shown by Qt at checkout")
    check.add_argument("--commit-id", help="operation shown by the Qt completion receipt, for committed")
    args = parser.parse_args()
    try:
        creating = args.command == "prepare"
        os.umask(0o077)
        root = fixture_location(args.run, prepare=creating)
        if creating:
            result = prepare(root)
        else:
            require(re.fullmatch("[0-9a-f]{32}", args.checkout_id) is not None, "invalid GUI checkout operation")
            require((args.phase == "committed" and args.commit_id is not None
                     and re.fullmatch("[0-9a-f]{32}", args.commit_id) is not None)
                    or (args.phase != "committed" and args.commit_id is None),
                    "--commit-id is required only for committed")
            result = verify(root, args.phase, args.checkout_id, args.commit_id)
        print(json.dumps(result, ensure_ascii=False, indent=2))
    except (OSError, KeyError, TypeError, ValueError, subprocess.SubprocessError, zipfile.BadZipFile) as error:
        parser.exit(1, f"Fixture check stopped: {error}\n")


if __name__ == "__main__":
    main()
