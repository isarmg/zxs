#!/usr/bin/env python3
"""Adopt one exact-SHA, fully executed CI push run when main receives that SHA.

Only public, unauthenticated GitHub reads are used. A lookup miss/error selects
the existing full validation path; it never constitutes validation success.
"""
from datetime import datetime, timezone
import http.client
import json
import os
from pathlib import Path
import re
import urllib.parse
import urllib.request

REPOSITORY = "isarmg/zxs"
REPOSITORY_ID = 1424437655
WORKFLOW_ID = 389550847
WORKFLOW_PATH = ".github/workflows/ci.yml"
API = f"https://api.github.com/repos/{REPOSITORY}"
MAX_CANDIDATES = 3
MAX_RESPONSE_BYTES = 1024 * 1024
REPORT_STEP = "Report adopted source validation"
CHECKOUT = "Run actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1"
WASM = "Install WASM standard library for cross-target checks"
SQLITE = "Install system SQLite SDK for unbundled validation"
SQLITE_CONSUMER = "Verify isolated system SQLite consumer"
CHROME = "Enable sandboxed Chrome for browser future runtime checks"
SECURITY_CACHE = "Cache verified security scanner"
FOUNDATION_EVIDENCE = "Retain exact foundation behaviour test evidence"
SUITES = (
    "rust-sources", "quality", "security", "workspace", "foundation-platform",
    "foundation-crypto", "foundation-data", "foundation-language",
    "foundation-async", "web", "contracts", "build-tools", "smb-protocol",
)
CONDITIONAL_STEPS = {WASM, SQLITE, SQLITE_CONSUMER, CHROME, SECURITY_CACHE, FOUNDATION_EVIDENCE}
JOB_STEPS = {
    "Static workflow and tooling regressions (no Rust)": {
        CHECKOUT, "Check workflows, source governance and script regressions",
    },
    "Production WASM build": {
        CHECKOUT, "Initialize isolated Cargo home",
        "Install repository Rust toolchain and WASM target",
        "Cache locally compiled Rust build helpers", "Build production Web distribution",
        "Retain production assets for checks in this run", "Retain browser template contract fixture",
    },
    "Extracted release browser smoke (Chrome)": {
        CHECKOUT, "Initialize isolated Cargo home", "Install repository Rust toolchain",
        "Fetch production Web assets from this workflow run",
        "Fetch browser template contract fixture from this run",
        "Verify runner Chrome and ChromeDriver", "Install CJK fonts for the Chinese screenshot",
        "Build and exercise extracted release packages",
        "Retain verified installation candidate (not a published release)",
        "Retain template browser result and exact input identity",
        "Retain real Web file manager screenshot", "Retain responsive Web viewport previews",
    },
    "smb-sdk / Samba 4.23.0 real SDK ABI and FFI tests": {
        CHECKOUT, "Initialize isolated Cargo home", "Install repository Rust toolchain",
        "Install official Ubuntu SDK build prerequisites",
        "Cache pinned configured Samba source and real libraries",
        "Build official SDK and validate the Rust VFS",
    },
}
for suite in SUITES:
    required = {CHECKOUT, "Initialize isolated Cargo home", "Install repository Rust toolchain",
                "Run locked test suite"}
    if suite in {"quality", "build-tools"} or suite.startswith("foundation-"):
        required.add(WASM)
    if suite == "foundation-data":
        required.update({SQLITE, SQLITE_CONSUMER})
    if suite == "foundation-language":
        required.add(CHROME)
    if suite == "security":
        required.add(SECURITY_CACHE)
    if suite == "foundation-async":
        required.add(FOUNDATION_EVIDENCE)
    JOB_STEPS[f"Tests ({suite})"] = required

# Artifacts stay in their original run. Bind each to that attempt's upload step,
# including on reruns, rather than accepting stale artifacts left by an attempt.
ARTIFACT_STEPS = {
    "web-dist": ("Production WASM build", "Retain production assets for checks in this run"),
    "csr-template-fixture": ("Production WASM build", "Retain browser template contract fixture"),
    "foundation-behaviour-evidence": ("Tests (foundation-async)", FOUNDATION_EVIDENCE),
    "installation-candidate-linux-web": ("Extracted release browser smoke (Chrome)",
                                          "Retain verified installation candidate (not a published release)"),
    "csr-template-browser": ("Extracted release browser smoke (Chrome)",
                             "Retain template browser result and exact input identity"),
    "web-screenshot": ("Extracted release browser smoke (Chrome)", "Retain real Web file manager screenshot"),
    "viewport-previews": ("Extracted release browser smoke (Chrome)", "Retain responsive Web viewport previews"),
}


class IncompleteEvidence(ValueError):
    pass


def require(condition, reason):
    if not condition:
        raise IncompleteEvidence(reason)


def timestamp(value):
    require(isinstance(value, str), "timestamp is not text")
    parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    require(parsed.tzinfo is not None, "timestamp has no timezone")
    return parsed


def positive_id(value):
    return type(value) is int and value > 0


def same_repository(value):
    return value["id"] == REPOSITORY_ID and value["full_name"] == REPOSITORY


def run_identity(run, sha):
    require(positive_id(run["id"]) and positive_id(run["run_attempt"]), "invalid run identity")
    require(run["workflow_id"] == WORKFLOW_ID and run["path"] == WORKFLOW_PATH,
            "different workflow identity")
    require(same_repository(run["repository"]) and same_repository(run["head_repository"]),
            "different repository or head repository")
    require(run["event"] == "push" and run["head_sha"] == sha and run["head_commit"]["id"] == sha,
            "different event or complete head SHA")


def completed_candidate(run, sha, current_id):
    run_identity(run, sha)
    require(run["id"] != current_id and isinstance(run["head_branch"], str)
            and run["head_branch"] and run["head_branch"] != "main"
            and not run["head_branch"].startswith("refs/"), "not a candidate branch push")
    require(run["status"] == "completed" and run["conclusion"] == "success",
            "candidate attempt is not successful and complete")


def verify_jobs(document, run):
    jobs = document["jobs"]
    require(document["total_count"] == len(JOB_STEPS) == len(jobs), "incomplete full job set")
    require({job["name"] for job in jobs} == JOB_STEPS.keys(), "different job set")
    require(len({job["id"] for job in jobs}) == len(jobs), "duplicate jobs")
    steps_by_job = {}
    for job in jobs:
        require(positive_id(job["id"]) and job["run_id"] == run["id"]
                and positive_id(job["run_attempt"]) and job["run_attempt"] == run["run_attempt"]
                and job["head_sha"] == run["head_sha"],
                "job belongs to a different SHA, run or attempt")
        require(job["status"] == "completed" and job["conclusion"] == "success", "job did not succeed")
        steps = {step["name"]: step for step in job["steps"]}
        required = JOB_STEPS[job["name"]]
        require(len(steps) == len(job["steps"]) and required <= steps.keys(), "missing or duplicate execution steps")
        optional = CONDITIONAL_STEPS - required if job["name"].startswith("Tests (") else set()
        for name, step in steps.items():
            require(isinstance(name, str), "step name is not text")
            allowed_skip = name in optional or name == REPORT_STEP or name.startswith("Post ")
            require(step["status"] == "completed" and (step["conclusion"] == "success"
                    or (allowed_skip and step["conclusion"] == "skipped")), "execution step did not succeed")
        steps_by_job[job["name"]] = steps
    return steps_by_job


def verify_artifacts(document, run, steps, now):
    artifacts = document["artifacts"]
    require(document["total_count"] == len(artifacts) == len(ARTIFACT_STEPS), "incomplete artifact set")
    require({item["name"] for item in artifacts} == ARTIFACT_STEPS.keys(), "different artifact set")
    for item in artifacts:
        origin = item["workflow_run"]
        require(positive_id(item["id"]) and origin["id"] == run["id"]
                and origin["repository_id"] == REPOSITORY_ID and origin["head_repository_id"] == REPOSITORY_ID
                and origin["head_sha"] == run["head_sha"] and origin["head_branch"] == run["head_branch"],
                "artifact has a different origin")
        require(item["expired"] is False and timestamp(item["expires_at"]) > now
                and type(item["size_in_bytes"]) is int and item["size_in_bytes"] > 0
                and re.fullmatch(r"sha256:[0-9a-f]{64}", item["digest"]), "artifact is unavailable or incomplete")
        job_name, step_name = ARTIFACT_STEPS[item["name"]]
        upload = steps[job_name][step_name]
        require(timestamp(upload["started_at"]) <= timestamp(item["created_at"])
                <= timestamp(item["updated_at"]) <= timestamp(upload["completed_at"]) <= now,
                "artifact does not belong to this attempt's successful upload")


def public_get(path):
    request = urllib.request.Request(API + path, headers={
        "Accept": "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28",
        "User-Agent": "zxs-exact-main-adoption",
    })
    with urllib.request.urlopen(request, timeout=10) as response:
        require(response.status == 200 and response.url == API + path, "unexpected API response or redirect")
        data = response.read(MAX_RESPONSE_BYTES + 1)
    require(len(data) <= MAX_RESPONSE_BYTES, "API response exceeds bound")
    return json.loads(data)


def find_adoption(environment, event, get=public_get, now=None):
    """Return (source run, explanation); absence always means full validation."""
    if environment.get("GITHUB_EVENT_NAME") != "push" or environment.get("GITHUB_REF") != "refs/heads/main":
        return None, "ordinary branch, PR or manual validation"
    now = now or datetime.now(timezone.utc)
    try:
        sha = environment["GITHUB_SHA"]
        require(re.fullmatch(r"[0-9a-f]{40}", sha), "incomplete SHA")
        require(environment["GITHUB_SERVER_URL"] == "https://github.com"
                and environment["GITHUB_REPOSITORY"] == REPOSITORY
                and environment["GITHUB_REPOSITORY_ID"] == str(REPOSITORY_ID)
                and environment["GITHUB_WORKFLOW_SHA"] == sha
                and environment["GITHUB_WORKFLOW_REF"] == f"{REPOSITORY}/{WORKFLOW_PATH}@refs/heads/main",
                "uncertain repository or workflow input")
        require(event["ref"] == "refs/heads/main" and event["after"] == sha
                and event["head_commit"]["id"] == sha and same_repository(event["repository"])
                and event["forced"] is False and event["deleted"] is False and event["created"] is False
                and re.fullmatch(r"[0-9a-f]{40}", event["before"])
                and event["before"] != "0" * 40 and event["before"] != sha, "not a main fast-forward push")
        current_id = int(environment["GITHUB_RUN_ID"])
        current = get(f"/actions/runs/{current_id}")
        run_identity(current, sha)
        require(current["id"] == current_id and current["head_branch"] == "main"
                and current["run_attempt"] == int(environment["GITHUB_RUN_ATTEMPT"]), "current run changed")
        query = urllib.parse.urlencode({"head_sha": sha, "event": "push", "status": "success",
                                        "per_page": MAX_CANDIDATES})
        candidates = get(f"/actions/workflows/{WORKFLOW_ID}/runs?{query}")["workflow_runs"]
        require(isinstance(candidates, list) and len(candidates) <= MAX_CANDIDATES, "unbounded run response")
        for listed in candidates:
            try:
                completed_candidate(listed, sha, current_id)
            except IncompleteEvidence:
                continue
            run = get(f"/actions/runs/{listed['id']}")
            completed_candidate(run, sha, current_id)
            require(run["id"] == listed["id"] and run["run_attempt"] == listed["run_attempt"], "candidate changed")
            steps = verify_jobs(get(f"/actions/runs/{run['id']}/attempts/{run['run_attempt']}/jobs?per_page=100"), run)
            verify_artifacts(get(f"/actions/runs/{run['id']}/artifacts?per_page=100"), run, steps, now)
            latest = get(f"/actions/runs/{run['id']}")
            completed_candidate(latest, sha, current_id)
            require(latest["id"] == run["id"] and latest["run_attempt"] == run["run_attempt"], "candidate rerun started")
            return run, "all 17 jobs, execution steps and original artifacts verified"
        return None, "no complete exact-SHA candidate push found in bounded lookup"
    except (LookupError, TypeError, ValueError, OSError, http.client.HTTPException) as error:
        return None, f"evidence unavailable: {type(error).__name__}: {error}"


def main():
    # Non-main invocations need neither a payload file nor network access.
    event = None
    try:
        if os.environ.get("GITHUB_EVENT_NAME") == "push" and os.environ.get("GITHUB_REF") == "refs/heads/main":
            event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text(encoding="utf-8"))
        run, reason = find_adoption(os.environ, event)
    except (LookupError, TypeError, ValueError, OSError) as error:
        run, reason = None, f"push payload unavailable: {type(error).__name__}: {error}"
    url = f"https://github.com/{REPOSITORY}/actions/runs/{run['id']}/attempts/{run['run_attempt']}" if run else ""
    message = (f"Adopted source validation: {url}\n\nSHA: `{run['head_sha']}`. "
               "Static and live security run again here. Artifacts retain their original run provenance."
               if run else f"Full validation selected: {reason}")
    print(message)
    with Path(os.environ["GITHUB_STEP_SUMMARY"]).open("a", encoding="utf-8") as summary:
        summary.write(f"\n{message}\n")
    # This is the final operation: a partial report must never grant adoption.
    with Path(os.environ["GITHUB_OUTPUT"]).open("a", encoding="utf-8") as output:
        output.write(f"reuse-source={'true' if run else 'false'}\nsource-run-url={url}\n")


if __name__ == "__main__":
    main()
