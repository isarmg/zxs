#!/usr/bin/env python3
"""Reuse only evidenced, unchanged foundation checks; uncertainty runs every suite."""
import hashlib
import http.client
import json
import os
from pathlib import Path
import re
import subprocess
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
REPOSITORY = "isarmg/zxs"
FOUNDATION = ("foundation-platform", "foundation-crypto", "foundation-data",
              "foundation-language", "foundation-async")
ALL = ("rust-sources", "quality", "security", "workspace", *FOUNDATION,
       "web", "contracts", "build-tools", "smb-protocol")
# Review the actual foundation consumers before updating this fingerprint.
# A mismatch is only a full-CI fallback, never a product/admission restriction.
REVIEWED_INPUTS = "36906cbe6a2fdd9f0661b0b6d01cf893a63cd1b61bc91b2144e9b0434cb53fca"
SELF = ".github/scripts/select-ci-suites.py"
SHA = re.compile(r"[0-9a-f]{40}")
DOCS = frozenset(("docs/EXTERNAL_TEST_SOURCES.md", "docs/architecture/read-projections.md",
                  "docs/validation/release-checklist.md", "docs/release.md",
                  "docs/validation/history-growth-benchmark.md"))



class FullRun(Exception):
    """A missing proof is a reason to run checks, not to skip them."""


def affected(path):
    """None means unknown. Returned suites have real consumers of this path."""
    if path in DOCS:
        return set()
    if path.startswith(("tools/history-scale-bench/", "test/history-scale-bench/")) and path.endswith((".rs", ".py")):
        # The language shard scans all src/tools Rust source for retired paths.
        return {"foundation-language"} if path.startswith("tools/") and path.endswith(".rs") else set()
    if path.endswith(".rs") and path.startswith(("src/engine/src/", "src/server/src/")):
        # Async checks the actual server's all-target consumer, including engine.
        return {"foundation-language", "foundation-async"}
    if path.endswith(".rs") and path.startswith(("test/engine/", "test/server/")):
        if path == "test/engine/integration/hash_table_compatibility.rs":
            return None  # Direct input to the foundation-data shard.
        return {"foundation-async"}
    return None


def git(root, *args):
    return subprocess.check_output(["git", *args], cwd=root, stderr=subprocess.PIPE)


def tree(root, commit):
    entries = {}
    for row in git(root, "ls-tree", "-r", "-z", commit).split(b"\0"):
        if row:
            metadata, path = row.split(b"\t", 1)
            mode, kind, digest = metadata.decode("ascii").split()
            entries[path.decode("utf-8")] = (mode, kind, digest)
    return entries


def input_fingerprint(entries):
    # Bind the allowlist to the reviewed check implementations/dependency surface.
    # Even future added consumers force full CI until this proof is reviewed again.
    digest = hashlib.sha256()
    for path, entry in sorted(entries.items()):
        if path != SELF and affected(path) is None:
            digest.update(json.dumps([path, *entry], ensure_ascii=True, separators=(",", ":")).encode())
            digest.update(b"\n")
    return digest.hexdigest()


def get_json(url):
    request = urllib.request.Request(url, headers={"Accept": "application/vnd.github+json",
                                                  "User-Agent": "zxs-ci-suite-selection"})
    # Public read only: no token, new permissions, credentials or writes.
    with urllib.request.urlopen(request, timeout=10) as response:
        data = response.read(2 * 1024 * 1024 + 1)
    if len(data) > 2 * 1024 * 1024:
        raise FullRun("public evidence response exceeds bound")
    return json.loads(data)


def complete_rows(document, key):
    if not isinstance(document, dict):
        raise FullRun("malformed evidence response")
    rows, count = document.get(key), document.get("total_count")
    if not isinstance(rows, list) or type(count) is not int or count != len(rows) or count > 100:
        raise FullRun("incomplete or paginated evidence response")
    if not all(isinstance(row, dict) for row in rows):
        raise FullRun("malformed evidence rows")
    return rows


def foundation_evidence(base, fetch):
    api = f"https://api.github.com/repos/{REPOSITORY}/actions/runs"
    runs = complete_rows(fetch(f"{api}?head_sha={base}&event=push&per_page=100"), "workflow_runs")
    for run in runs:
        if (type(run.get("id")) is not int or run["id"] <= 0
                or type(run.get("run_attempt")) is not int or run["run_attempt"] <= 0
                or run.get("head_sha") != base or run.get("event") != "push"
                or not all(isinstance(run.get(key), str) for key in ("head_branch", "path", "status"))):
            raise FullRun("malformed exact-base run identity")
    candidates = [run for run in runs if run.get("head_sha") == base
                  and run.get("head_branch") == "main" and run.get("event") == "push"
                  and run.get("path") == ".github/workflows/ci.yml"
                  and type(run.get("id")) is int and run["id"] > 0
                  and type(run.get("run_attempt")) is int and run["run_attempt"] > 0]
    if not candidates:
        raise FullRun("exact base has no CI evidence")
    # Do not search ancestors, older attempts, similarly named checks or partial pages.
    run = max(candidates, key=lambda value: value["id"])
    if run["status"] != "completed":
        raise FullRun("latest exact-base CI is not complete")
    jobs = complete_rows(fetch(f"{api}/{run['id']}/jobs?filter=latest&per_page=100"), "jobs")
    evidence = {}
    for suite in FOUNDATION:
        matches = [job for job in jobs if job.get("name") == f"Tests ({suite})"]
        if len(matches) != 1:
            raise FullRun("base evidence lacks one exact foundation job")
        job = matches[0]
        if (type(job.get("run_id")) is not int or type(job.get("run_attempt")) is not int
                or job.get("run_id") != run["id"] or job.get("run_attempt") != run["run_attempt"]
                or job.get("head_sha") != base or job.get("status") != "completed"
                or job.get("conclusion") != "success" or type(job.get("id")) is not int
                or job["id"] <= 0):
            raise FullRun("base foundation checks did not all succeed")
        evidence[suite] = job["id"]
    return {"run_id": run["id"], "run_attempt": run["run_attempt"], "jobs": evidence}


def select(root, event_name, event, head, ref, *, fetch=get_json, reviewed=REVIEWED_INPUTS):
    receipt = {"head": head, "base": event.get("before"), "event": event_name,
               "suites": list(ALL), "reused": [], "reason": "full CI default"}
    try:
        if event_name != "push" or ref != "refs/heads/main":
            raise FullRun("only main push can reuse evidence; PR/dispatch/other events run full CI")
        base = event.get("before")
        if (not isinstance(base, str) or SHA.fullmatch(base) is None or base == "0" * 40
                or not isinstance(head, str) or SHA.fullmatch(head) is None
                or event.get("after") != head or not isinstance(event.get("repository"), dict)
                or event["repository"].get("full_name") != REPOSITORY):
            raise FullRun("missing or mismatched exact push identities")
        if git(root, "rev-parse", "HEAD").decode().strip() != head:
            raise FullRun("checkout is not the exact event head")
        git(root, "merge-base", "--is-ancestor", base, head)
        before, after = tree(root, base), tree(root, head)
        fields = git(root, "diff", "--name-only", "--no-renames", "-z", base, head, "--").split(b"\0")
        paths = [field.decode("utf-8") for field in fields if field]
        receipt["changed_paths"] = paths
        if not paths:
            raise FullRun("empty diff has no reviewed reuse case")
        keep = set()
        for path in paths:
            consumers = affected(path)
            if consumers is None:
                raise FullRun(f"unreviewed input changed: {path}")
            for entries in (before, after):
                if path in entries and entries[path][:2] != ("100644", "blob"):
                    raise FullRun(f"non-regular input changed: {path}")
            keep.update(consumers)
        for entries in (before, after):
            if any(affected(path) is not None and entry[:2] != ("100644", "blob")
                   for path, entry in entries.items()):
                raise FullRun("reviewed paths must remain regular tracked files")
        fingerprint = input_fingerprint(after)
        receipt["reviewed_input_fingerprint"] = fingerprint
        if fingerprint != reviewed or input_fingerprint(before) != reviewed:
            raise FullRun("foundation dependency/check input surface needs review; running all suites")
        receipt["evidence"] = foundation_evidence(base, fetch)
        reused = [suite for suite in FOUNDATION if suite not in keep]
        receipt.update(suites=[suite for suite in ALL if suite not in reused], reused=reused,
                       reason="exact base foundation success and reviewed unchanged inputs")
    except (FullRun, OSError, http.client.HTTPException, subprocess.CalledProcessError, ValueError, UnicodeError) as error:
        receipt["reason"] = str(error)
    return receipt


def main():
    try:
        event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
        if not isinstance(event, dict):
            raise ValueError("event must be an object")
    except (KeyError, OSError, ValueError) as error:
        event = {}
        print(f"No usable event proof: {error}")
    receipt = select(ROOT, os.environ.get("GITHUB_EVENT_NAME", ""), event,
                     os.environ.get("GITHUB_SHA", ""), os.environ.get("GITHUB_REF", ""))
    print(json.dumps(receipt, indent=2, sort_keys=True))
    with open(os.environ["GITHUB_OUTPUT"], "a") as output:
        output.write("suites=" + json.dumps(receipt["suites"], separators=(",", ":")) + "\n")
    if summary := os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(summary, "a") as output:
            output.write("### CI suite selection\n\n```json\n" + json.dumps(receipt, indent=2) + "\n```\n")


if __name__ == "__main__":
    main()
