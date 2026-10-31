#!/usr/bin/env python3
"""Select a bounded validation matrix; scale execution requires a reviewed freeze."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re

BASELINE = "e2f33d9882cf7263412d9370b6a703e0b216357d"
ARCHIVED_CANDIDATE = "209f31a4122174129bcf40b56c0598ac61e66352"
PROFILES = ("small-selftest", "boundaries", "1000", "10000")


def commit_sha(value: object) -> str:
    if not isinstance(value, str) or re.fullmatch(r"[0-9a-f]{40}", value) is None:
        raise ValueError("expected an exact lowercase 40-digit commit SHA")
    return value


def reviewed_candidate(value: object) -> str:
    sha = commit_sha(value)
    if sha == BASELINE:
        raise ValueError("candidate must differ from the comparison baseline")
    if sha == ARCHIVED_CANDIDATE:
        raise ValueError("archived experimental candidate is unsupported by the current-main adapter")
    return sha


def unique_fields(pairs: list[tuple[str, object]]) -> dict:
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate acceptance plan field: {key}")
        result[key] = value
    return result


def load_plan(path: Path) -> dict:
    plan = json.loads(path.read_text(encoding="utf-8"), object_pairs_hook=unique_fields)
    fields = {"schema_version", "baseline_sha", "reviewed_candidate_sha", "frozen_candidate_sha", "ten_thousand_review", "profiles"}
    if not isinstance(plan, dict) or set(plan) != fields or type(plan["schema_version"]) is not int or plan["schema_version"] != 1:
        raise ValueError("unsupported acceptance plan schema")
    if commit_sha(plan["baseline_sha"]) != BASELINE:
        raise ValueError("the historical comparison baseline must remain fixed")
    if plan["reviewed_candidate_sha"] is not None:
        reviewed_candidate(plan["reviewed_candidate_sha"])
    frozen = plan["frozen_candidate_sha"]
    if frozen is not None and commit_sha(frozen) != plan["reviewed_candidate_sha"]:
        raise ValueError("frozen candidate must equal the source-reviewed candidate")
    if plan["profiles"] != list(PROFILES):
        raise ValueError("unsupported profile set; 100k is not authorized")
    review = plan["ten_thousand_review"]
    if review is not None:
        if not isinstance(review, dict) or set(review) != {"candidate_sha", "validation_run_id", "reviewed_profiles", "rationale"}:
            raise ValueError("invalid 10k review schema")
        if commit_sha(review["candidate_sha"]) != frozen or review["reviewed_profiles"] != ["boundaries", "1000"]:
            raise ValueError("10k review must cover both initial profiles for this frozen candidate")
        positive_id(review["validation_run_id"])
        if not isinstance(review["rationale"], str) or not review["rationale"].strip():
            raise ValueError("10k needs a recorded result and resource trend review")
    return plan


def positive_id(value: object) -> str:
    if not isinstance(value, str) or not value.isascii() or not value.isdigit() or int(value) < 1:
        raise ValueError("expected a positive GitHub run ID")
    return value


def build_source(mode: str, run_id: str, carrier_sha: str, current_run: str, current_sha: str) -> dict:
    if bool(run_id) != bool(carrier_sha):
        raise ValueError("binary reuse needs both build run ID and build carrier SHA")
    if not run_id and mode != "selftest":
        raise ValueError("scale validation must reuse the verified selftest binary builds")
    return {
        "build_required": not bool(run_id),
        "build_run_id": positive_id(run_id or current_run),
        "build_carrier_sha": commit_sha(carrier_sha or current_sha),
    }


def validate_reuse_run(run: dict, run_id: str, carrier_sha: str, *, require_success: bool = True) -> None:
    if str(run.get("id")) != positive_id(run_id) or run.get("head_sha") != commit_sha(carrier_sha):
        raise ValueError("reused build run identity mismatch")
    if run.get("path") != ".github/workflows/history-scale.yml" or run.get("event") != "workflow_dispatch":
        raise ValueError("reused build must come from the dedicated dispatched validation workflow")
    if run.get("status") != "completed" or (require_success and run.get("conclusion") != "success"):
        raise ValueError("binary reuse requires a successfully completed selftest run")


def validate_build_jobs(run: dict, jobs: dict, run_id: str, carrier_sha: str, *, candidate_only: bool = False) -> None:
    validate_reuse_run(run, run_id, carrier_sha, require_success=False)
    names = {"build (candidate)"} if candidate_only else {"build (baseline)", "build (candidate)"}
    selected = [row for row in jobs["jobs"] if row["name"] in names]
    if len(selected) != len(names) or {row["name"] for row in selected} != names or any(row["status"] != "completed" or row["conclusion"] != "success" for row in selected):
        message = "the original candidate binary build job" if candidate_only else "both original binary build jobs"
        raise ValueError(f"{message} must have succeeded")


def validate_reused_selftest(selection: dict, summary: dict, run_id: str, carrier_sha: str, candidate_sha: str, *, candidate_only: bool = False) -> None:
    if selection.get("candidate_only", False) is not candidate_only:
        raise ValueError("selftest source matrix differs from the selected experiment")
    if selection.get("mode") != "selftest":
        raise ValueError("reuse must reference a successful bounded selftest")
    if selection.get("build_run_id") != run_id or selection.get("build_carrier_sha") != carrier_sha:
        raise ValueError("selftest selection/build identity mismatch")
    if selection.get("baseline_sha") != BASELINE or selection.get("candidate_sha") != commit_sha(candidate_sha):
        raise ValueError("selftest selection source revisions mismatch")
    if selection.get("matrix") != {"include": [{"profile": "small-selftest"}]}:
        raise ValueError("reused run did not select the bounded selftest")
    for field in ("all_requested_samples_passed", "cohort_complete", "requested_matrix_matches"):
        if summary.get(field) is not True:
            raise ValueError("reused selftest did not fully pass")
    samples = summary.get("samples")
    labels = ("candidate",) if candidate_only else ("baseline", "candidate")
    expected = {(label, mode) for label in labels for mode in ("timing", "counts")}
    if not isinstance(samples, list) or len(samples) != len(expected) or {(row.get("label"), row.get("mode")) for row in samples} != expected:
        raise ValueError("reused selftest sample matrix mismatch")
    for row in samples:
        stages = row.get("stages", {})
        if row.get("operations") != 8 or row.get("status") != "passed" or set(stages) != {"build", "reopen", "recovery"} or any(value.get("status") != "passed" for value in stages.values()):
            raise ValueError("reused selftest needs every selected lifecycle stage completed")


def profile_settings(plan: dict, name: str) -> dict:
    if name not in plan["profiles"]:
        raise ValueError(f"unsupported validation profile: {name}")
    if plan["reviewed_candidate_sha"] is None:
        raise ValueError("no current-main candidate selected; execution remains disabled")
    small = name == "small-selftest"
    if not small and plan["frozen_candidate_sha"] is None:
        raise ValueError("scale validation is pending the parent's final frozen SHA authorization")
    if name == "10000" and plan["ten_thousand_review"] is None:
        raise ValueError("10k validation is pending the 1k and boundary result/resource review")
    sizes = [8] if small else ([255, 256, 257, 1023, 1024, 1025] if name == "boundaries" else [int(name)])
    return {
        "baseline_sha": plan["baseline_sha"],
        "candidate_sha": plan["reviewed_candidate_sha"] if small else plan["frozen_candidate_sha"],
        "sizes": sizes,
        "repeats": 1 if small or name == "boundaries" else 2,
        "counts_repeats": 1,
        "counts_sizes": [1025] if name == "boundaries" else sizes,
        "phase_seconds": 60 if small else 600,
        "cohort_seconds": 240 if small else 2400,
    }


def select(plan: dict, mode: str, *, candidate_only: bool = False) -> dict:
    if mode not in {"selftest", "acceptance", "10000"}:
        raise ValueError(f"unsupported controller mode: {mode}")
    names = {"selftest": ["small-selftest"], "acceptance": ["boundaries", "1000"], "10000": ["10000"]}[mode]
    if candidate_only:
        if mode == "10000":
            raise ValueError("current-only experiment is limited to selftest and boundaries")
        names = ["small-selftest"] if mode == "selftest" else ["boundaries"]
    settings = {name: profile_settings(plan, name) for name in names}
    candidate = next(iter(settings.values()))["candidate_sha"]
    return {
        "mode": mode,
        "baseline_sha": plan["baseline_sha"],
        "candidate_sha": candidate,
        "candidate_frozen": plan["frozen_candidate_sha"] is not None,
        "candidate_only": candidate_only,
        "settings": settings,
        "matrix": {"include": [{"profile": name} for name in names]},
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--plan", type=Path, required=True)
    parser.add_argument("--mode", choices=("selftest", "acceptance", "10000"), required=True)
    parser.add_argument("--candidate-only", action="store_true")
    parser.add_argument("--build-run-id", default="")
    parser.add_argument("--build-carrier-sha", default="")
    parser.add_argument("--selftest-run-id", default="")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--github-output", type=Path)
    args = parser.parse_args()
    selected = select(load_plan(args.plan), args.mode, candidate_only=args.candidate_only)
    selected.update(build_source(args.mode, args.build_run_id, args.build_carrier_sha,
                                 os.environ["GITHUB_RUN_ID"], os.environ["GITHUB_SHA"]))
    selected["selftest_run_id"] = positive_id(os.environ["GITHUB_RUN_ID"] if args.mode == "selftest" else (args.selftest_run_id or selected["build_run_id"]))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("x", encoding="utf-8") as destination:
        json.dump(selected, destination, indent=2, sort_keys=True)
        destination.write("\n")
    if args.github_output is not None:
        with args.github_output.open("a", encoding="utf-8") as destination:
            for name in ("baseline_sha", "candidate_sha", "matrix", "build_required", "build_run_id", "build_carrier_sha", "selftest_run_id"):
                value = selected[name]
                destination.write(f"{name}={json.dumps(value) if isinstance(value, (dict, bool)) else value}\n")
    print(json.dumps(selected, sort_keys=True))


if __name__ == "__main__":
    main()
