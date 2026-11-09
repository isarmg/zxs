#!/usr/bin/env python3
"""Import approved SQLite materials on GitHub; create blobs/tree, never commit/ref."""
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time
import tomllib

from sqlite_import.artifacts import (GitHub, PATCH, EVIDENCE, REPOSITORY, SOURCE_BASE,
                                     SOURCE_RUN, approved_inputs, object_sha, read_json)
from sqlite_import.bindings import prepare as prepare_bindings
from sqlite_upgrade.archive import IDENTITY, NAME, SHA256, URL, VERSION, relative, sha256
from sqlite_upgrade.upgrade import DIRECTORY, OLD_ORIGIN, PROVENANCE, TEST_ROOT

ROOT = Path(__file__).resolve().parents[2]
LIMIT_SECONDS = 11 * 60
START = time.monotonic()


def run(command):
    remaining = int(LIMIT_SECONDS - (time.monotonic() - START))
    if remaining <= 0:
        raise TimeoutError("SQLite import exceeded its internal budget")
    # The scoped write token is held only by the API client, not child processes.
    env = {key: value for key, value in os.environ.items() if not key.endswith("_TOKEN")}
    return subprocess.run(command, cwd=ROOT, env=env, check=True, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, timeout=remaining).stdout


def require_workflow():
    expected = {"GITHUB_ACTIONS": "true", "GITHUB_REPOSITORY": REPOSITORY,
                "GITHUB_EVENT_NAME": "workflow_dispatch", "GITHUB_REF": "refs/heads/main",
                "GITHUB_WORKFLOW_REF": REPOSITORY + "/.github/workflows/sqlite-import.yml@refs/heads/main",
                "GITHUB_API_URL": "https://api.github.com", "CARGO_NET_OFFLINE": "true"}
    if len(sys.argv) != 1 or any(os.environ.get(k) != v for k, v in expected.items()):
        raise ValueError("only the exact manual main-branch import job is allowed")
    head = object_sha(os.environ.get("GITHUB_SHA"))
    if (run(["git", "rev-parse", "HEAD"]).decode().strip() != head
            or os.environ.get("GITHUB_WORKFLOW_SHA") != head
            or run(["git", "status", "--porcelain=v1", "--untracked-files=all"])):
        raise ValueError("import requires the exact clean dispatched checkout")
    if os.environ.get("RUSTFLAGS") or os.environ.get("CARGO_ENCODED_RUSTFLAGS"):
        raise ValueError("unexpected compiler overrides")
    run(["python3", ".github/scripts/development-ci.py", "check"])
    return head


def local_tree(reference):
    data = run(["git", "ls-tree", "-r", "-z", "--full-tree", reference])
    result = {}
    for row in data.rstrip(b"\0").split(b"\0"):
        fields, path = row.split(b"\t", 1)
        mode, kind, digest = fields.decode("ascii").split()
        path = path.decode("utf-8")
        if path in result:
            raise ValueError("duplicate index tree path")
        result[path] = (mode, kind, object_sha(digest))
    return result


def source_baseline(api, head, current, evidence):
    comparison = api.request("compare/" + SOURCE_BASE + "..." + head + "?per_page=1")
    if (comparison["status"] not in ("ahead", "identical")
            or comparison["merge_base_commit"]["sha"] != SOURCE_BASE
            or comparison["base_commit"]["sha"] != SOURCE_BASE):
        raise ValueError("dispatched main does not descend from the exact artifact base")
    commit = api.request("git/commits/" + SOURCE_BASE)
    if commit["sha"] != SOURCE_BASE:
        raise ValueError("source commit identity differs")
    original = api.tree(commit["tree"]["sha"])
    changed = sorted(p for p in original.keys() | current.keys()
                     if original.get(p) != current.get(p))
    if len(changed) > 2048:
        raise ValueError("latest-main difference exceeds the reviewed import budget")
    evidence["base_delta"] = [{"path": p, "source": original.get(p), "current": current.get(p)}
                              for p in changed]
    return original


def require_paths(source, previous, original, current):
    mappings = []
    for path in (ROOT / "compliance/external-tests").glob("*.json"):
        for owner in read_json(path.read_bytes())["owners"]:
            if owner["directory"] == DIRECTORY:
                mappings.append((path.relative_to(ROOT).as_posix(), owner))
    if len(mappings) != 1:
        raise ValueError("predecessor external-test owner is not unique")
    mapping, mapping_owner = mappings[0]
    # manage.verify has authenticated the catalog's origin and content totals.
    # The mapping stores only raw fields; the artifact archives the FULL receipt.
    owner = previous["external_tests"][DIRECTORY]
    derived = ("origin_directory", "file_count", "bytes")
    raw_owner = {key: value for key, value in owner.items() if key not in derived}
    if mapping_owner != raw_owner:
        raise ValueError("raw predecessor mapping differs from the verified catalog receipt")
    packages = source["upgrade_plan"]["packages"]
    if (len(packages) != 1 or packages[0]["directory"] != DIRECTORY
            or packages[0]["package"] != previous["packages"][DIRECTORY]
            or packages[0]["external_tests"] != owner):
        raise ValueError("artifact predecessor receipts differ from current main")
    exact = {"Cargo.lock", "src/platform/Cargo.toml",
             "src/foundation/data-storage/sqlite-access/Cargo.toml",
             "compliance/provenance/origins.json", "compliance/rust-sources.json",
             "docs/THIRD_PARTY_RUST_SOURCES.md", "docs/architecture/foundation-consumers.md",
             "compliance/external-tests/sqlite-0.39.1.json", mapping}
    prefixes = (DIRECTORY + "/", PROVENANCE + "/", TEST_ROOT + "/",
                "compliance/licenses/upstream/" + IDENTITY + "/", "docs/upstream/" + IDENTITY + "/")
    paths = source["patch"]["paths"]
    if (not isinstance(paths, list) or not 1 <= len(paths) <= 512
            or any(not isinstance(p, str) or not re.fullmatch(r"[A-Za-z0-9_./-]+", p) for p in paths)
            or paths != sorted(set(paths))):
        raise ValueError("invalid bounded patch path inventory")
    for path in paths:
        relative(path)
        if path not in exact and not path.startswith(prefixes):
            raise ValueError("patch escapes the exact SQLite source/receipt scope: " + path)
        if original.get(path) != current.get(path):
            raise ValueError("patch path changed since its exact source base: " + path)
        if path in current and current[path][:2] != ("100644", "blob"):
            raise ValueError("patch predecessor is not ordinary source: " + path)
    return paths, owner


def patch_entries(paths):
    rows = run(["git", "diff", "--cached", "--raw", "--abbrev=40", "--no-renames",
                "-z", "HEAD", "--"]).split(b"\0")
    if rows[-1] != b"" or (len(rows) - 1) % 2:
        raise ValueError("invalid staged diff shape")
    entries = []
    for index in range(0, len(rows) - 1, 2):
        old_mode, new_mode, old_sha, new_sha, status = rows[index].decode("ascii").split()
        path = rows[index + 1].decode("utf-8")
        if (old_mode not in (":000000", ":100644") or new_mode not in ("000000", "100644")
                or status not in ("A", "M", "D")):
            raise ValueError("patch changes a mode, symlink or unsupported object")
        object_sha(old_sha)
        object_sha(new_sha)
        entries.append({"path": path, "mode": "100644", "type": "blob",
                        "sha": None if status == "D" else new_sha})
    if sorted(row["path"] for row in entries) != paths:
        raise ValueError("applied patch paths do not match the authenticated inventory")
    return entries


def verify_successor(manage, source, release, previous, owner):
    run(["python3", "tools/rust-sources/manage.py", "verify", "--metadata"])
    run(["python3", "tools/rust-sources/architecture.py", "verify"])
    current = read_json((ROOT / "compliance/rust-sources.json").read_bytes())
    successor = current["packages"][DIRECTORY]
    if (successor["version"] != VERSION
            or successor["upstream"] != {"name": NAME, "version": VERSION,
                                        "archive_url": URL, "archive_sha256": SHA256}
            or current["superseded"][OLD_ORIGIN]["package"] != previous["packages"][DIRECTORY]
            or current["superseded"][OLD_ORIGIN]["external_tests"] != owner):
        raise ValueError("actual successor/provenance chain differs")
    if any(row["upstream"]["name"] == NAME and row["upstream"]["version"] != VERSION
           for row in [*current["packages"].values(), *current.get("merges", [])]):
        raise ValueError("an affected active SQLite origin remains")
    manifest = tomllib.loads((ROOT / DIRECTORY / "Cargo.toml").read_text())
    if manifest["package"]["version"] != VERSION:
        raise ValueError("actual SQLite manifest is not the approved successor")
    actual = manage.files(ROOT / DIRECTORY)
    if actual != source["new_source_files_sha256"]:
        raise ValueError("installed SQLite files differ from the successful source evidence")
    for prefix in ("sqlite3/", "bindgen-bindings/"):
        if ({p: value for p, value in actual.items() if p.startswith(prefix)}
                != {p: value for p, value in release.inventory.items() if p.startswith(prefix)}):
            raise ValueError("complete official native sources/bindings differ")
    license_path = ROOT / "compliance/licenses/upstream" / IDENTITY / "LICENSE"
    if license_path.read_bytes() != release.files["LICENSE"]:
        raise ValueError("complete official license differs")


def publish_objects(api, head, before, entries, expected_tree, evidence):
    api.require_main(head)
    known = {row[2] for row in before.values() if row[1] == "blob"}
    for entry in entries:
        digest = entry["sha"]
        if digest is not None and digest not in known:
            api.create_blob(run(["git", "cat-file", "blob", digest]), digest)
            known.add(digest)
    # No commit or ref exists at any point; stale runs can leave only unreachable objects.
    api.require_main(head)
    base_tree = run(["git", "rev-parse", "HEAD^{tree}"]).decode().strip()
    tree = api.create_tree(base_tree, entries, expected_tree)
    if api.tree(tree) != local_tree(expected_tree):
        raise ValueError("created tree readback differs from the exact candidate")
    api.require_main(head)
    evidence["tree"] = tree
    evidence["parent"] = head


def main():
    if os.environ.get("GITHUB_ACTIONS") != "true":
        raise SystemExit("Refusing local execution; use the reviewed GitHub import workflow.")
    token = os.environ.pop("GH_TOKEN", "")
    directory = Path(os.environ["RUNNER_TEMP"]) / "zxs-sqlite-import"
    directory.mkdir(exist_ok=False)
    evidence = {"status": "failed", "stage": "workflow-admission",
                "source_run": SOURCE_RUN, "source_base": SOURCE_BASE,
                "patch_artifact_id": PATCH[0], "patch_zip_sha256": PATCH[2],
                "source_evidence_id": EVIDENCE[0], "evidence_zip_sha256": EVIDENCE[2],
                "remote_write_attempted": False, "created_objects": [],
                "not_executed": ["compilation", "tests", "security audit", "commit",
                                 "ref update", "push", "deployment"]}

    def deadline(signum, frame):
        raise TimeoutError("SQLite import exceeded its internal budget")

    signal.signal(signal.SIGALRM, deadline)
    signal.alarm(LIMIT_SECONDS)
    try:
        head = require_workflow()
        evidence["base_commit"] = head
        api = GitHub(token, evidence)
        api.require_main(head)
        before = local_tree("HEAD")
        sys.path.insert(0, str(ROOT / "tools/rust-sources"))
        import manage
        evidence["stage"] = "current-source-verification"
        run(["python3", "tools/rust-sources/manage.py", "verify", "--metadata"])
        previous = read_json((ROOT / "compliance/rust-sources.json").read_bytes())
        if previous["packages"][DIRECTORY]["version"] == VERSION:
            paths = prepare_bindings(api, head, ROOT, manage, run, previous, evidence)
            patch = run(["git", "diff", "--cached", "--binary", "--full-index", "--"])
            (directory / "candidate.patch").write_bytes(patch)
            entries = patch_entries(paths)
        else:
            evidence["task"] = "import-approved-sqlite-upgrade-artifact"
            original = source_baseline(api, head, before, evidence)
            evidence["stage"] = "fixed-artifact-and-original-source-admission"
            patch, source, release = approved_inputs(api)
            evidence["patch"] = source["patch"]
            paths, owner = require_paths(source, previous, original, before)
            if manage.files(ROOT / DIRECTORY) != source["old_source_files_sha256"]:
                raise ValueError("complete predecessor source differs from artifact evidence")
            patch_path = directory / "candidate.patch"
            patch_path.write_bytes(patch)
            evidence["stage"] = "exact-patch-application"
            run(["git", "apply", "--check", "--index", "--binary", "--whitespace=nowarn", str(patch_path)])
            run(["git", "apply", "--index", "--binary", "--whitespace=nowarn", str(patch_path)])
            entries = patch_entries(paths)
            evidence["stage"] = "successor-source-and-license-verification"
            verify_successor(manage, source, release, previous, owner)
        evidence["candidate_patch"] = {"sha256": sha256(patch), "bytes": len(patch), "paths": paths}
        if (run(["git", "diff", "--name-only", "-z", "--"])
                or run(["git", "ls-files", "--others", "--exclude-standard", "-z"])):
            raise ValueError("verification changed source outside the authenticated index")
        if patch_entries(paths) != entries:
            raise ValueError("verification changed the authenticated staged patch")
        expected = object_sha(run(["git", "write-tree"]).decode().strip())
        evidence["expected_tree"] = expected
        evidence["stage"] = "git-object-upload"
        publish_objects(api, head, before, entries, expected, evidence)
        evidence["status"] = "tree-ready-for-coordinator"
        evidence["stage"] = "complete"
        print(json.dumps({"parent": head, "tree": expected, "task": evidence["task"], "source_run": SOURCE_RUN,
                          "patch_sha256": sha256(patch), "changed_paths": paths}, sort_keys=True))
        print("Verified source import only; no compilation, tests, audit, commit or ref update.")
    except Exception as error:
        evidence["error"] = str(error)
        if isinstance(error, subprocess.CalledProcessError):
            evidence["command_error"] = error.stderr.decode("utf-8", errors="replace")[-16384:]
        raise
    finally:
        signal.alarm(0)
        evidence["elapsed_seconds"] = round(time.monotonic() - START, 3)
        (directory / "result.json").write_text(json.dumps(evidence, indent=2, sort_keys=True) + "\n")


if __name__ == "__main__":
    main()
