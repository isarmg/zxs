"""Admit one untraced growth tier from completed, source-bound CI evidence."""

from __future__ import annotations

from datetime import datetime, timezone
import hashlib
import math
from pathlib import Path

from controller import commit_sha, positive_id, validate_reuse_run
from summarize import read_json, summarize_driver


STAGES = ("build", "reopen", "recovery")
TIERS = {
    "growth-2048": (2048, "boundaries", 1, 600, 2400),
    "growth-4096": (4096, "growth-2048", 1, 600, 2400),
    "growth-10000": (10000, "growth-4096", 2, 1800, 3600),
    "growth-20000": (20000, "growth-10000", 2, 1800, 3600),
}
RESOURCES = {"rss_mib": 2048, "disk_mib": 4096, "cohort_disk_mib": 8192, "minimum_free_mib": 8192}
SAFETY = 1.5
MIN_EXPONENT = {"wall": 1.25, "rss": 1.0, "disk": 1.0}
EXPONENT_MARGIN = 0.15
COHORT_FINISH_SECONDS = 60


def load_policy(path: Path) -> dict:
    value = read_json(path.read_text(encoding="utf-8"))
    fields = {"schema_version", "source_sha", "source_tree", "binary_sha256", "build_run_id", "build_carrier_sha",
              "boundary_run_id", "boundary_carrier_sha"}
    if not isinstance(value, dict) or set(value) != fields or type(value["schema_version"]) is not int or value["schema_version"] != 1:
        raise ValueError("unsupported growth policy schema")
    for field in ("source_sha", "source_tree", "build_carrier_sha", "boundary_carrier_sha"):
        commit_sha(value[field])
    for field in ("build_run_id", "boundary_run_id"):
        positive_id(value[field])
    from verify_build import required_digest
    required_digest(value["binary_sha256"])
    return value


def checked_summary(summary: dict, environment: dict, profile: str, policy: dict, evidence: Path) -> list[dict]:
    for field in ("cohort_complete", "requested_matrix_matches", "all_requested_samples_passed"):
        if summary.get(field) is not True:
            raise ValueError(f"predecessor evidence is not complete: {field}")
    if environment["validation_profile"] != profile or environment["source_matrix"] != ["candidate"]:
        raise ValueError("predecessor profile/source matrix differs")
    source = environment["sources"]["candidate"]
    if source["head"] != policy["source_sha"] or source["precompiled_provenance"]["binary_sha256"] != policy["binary_sha256"]:
        raise ValueError("predecessor source/binary differs")
    if environment["expected_build_identity"] != {"GITHUB_RUN_ID": policy["build_run_id"], "GITHUB_SHA": policy["build_carrier_sha"]}:
        raise ValueError("predecessor original build identity differs")
    if profile == "boundaries":
        expected = {(size, "timing", 1) for size in (255, 256, 257, 1023, 1024, 1025)} | {(1025, "counts", 1)}
    else:
        size, _, repeats, _, _ = TIERS[profile]
        expected = {(size, "timing", repeat) for repeat in range(1, repeats + 1)}
    samples = summary["samples"]
    if len(samples) != len(expected) or {(row["operations"], row["mode"], row["repeat"]) for row in samples} != expected:
        raise ValueError("predecessor sample matrix is incomplete or duplicated")
    for row in samples:
        if row["label"] != "candidate" or row["status"] != "passed" or set(row["stages"]) != set(STAGES):
            raise ValueError("predecessor has a failed or missing stage")
        if (row["sha"], row["binary_sha256"], row["profile"], row["query_protocol"]) != (policy["source_sha"], policy["binary_sha256"], profile, "separate-outcome-health"):
            raise ValueError("predecessor sample source, binary or protocol differs")
        for name, stage in row["stages"].items():
            monitor = stage["supervisor"]
            if stage["status"] != "passed" or monitor["returncode"] != 0 or monitor["monitor_error"] or monitor["finalization_errors"] or monitor["budget_stop_reason"]:
                raise ValueError("predecessor exit, monitoring or budget evidence failed")
            if monitor["wall_seconds"] <= 0 or monitor["time_max_process_rss_bytes"] <= 0:
                raise ValueError("predecessor has no positive runtime/resource evidence")
            driver = stage["driver"]
            if driver["configuration"]["fixture_mode"] != "fixed-body" or driver["configuration"]["query_protocol"] != "separate-outcome-health":
                raise ValueError("predecessor fixture or outcome/health oracle differs")
            complete = driver["completion"]
            if complete["verified"] is not True or any(complete[key] != row["operations"] for key in ("operations", "actual_total_receipts", "actual_total_events")):
                raise ValueError("predecessor original ID/event oracle is incomplete")
            raw = evidence / "samples" / f"{row['operations']:06d}-{row['mode']}-{row['repeat']:02d}-candidate" / f"{name}.driver.jsonl"
            actual = summarize_driver(raw, name, row["operations"], row["mode"], baseline=False, required_protocol="separate-outcome-health")
            if actual != driver:
                raise ValueError("predecessor summary differs from the complete raw driver protocol")
    return samples


def observed(samples: list[dict], size: int) -> dict:
    selected = [sample for sample in samples if sample["operations"] == size and sample["mode"] == "timing"]
    monitors = [sample["stages"][name]["supervisor"] for sample in selected for name in STAGES]
    if not selected:
        raise ValueError("no untraced predecessor samples")
    rss = max(max(item["time_max_process_rss_bytes"], item["sampled_tree_peak_rss_bytes"]) for item in monitors)
    rss = max(rss, max(row["vmrss_kib"] * 1024 for sample in selected for stage in sample["stages"].values() for row in stage["driver"]["telemetry"]))
    disk = max(max([item["final_budget_observation"]["sample_allocated_bytes"],
                    *(row["allocated_bytes"] for row in item["disk_samples"])]) for item in monitors)
    disk = max(disk, max(sample["total_sample_inventory"]["allocated_bytes"] for sample in selected))
    return {"size": size, "wall": {name: max(sample["stages"][name]["supervisor"]["wall_seconds"] for sample in selected) for name in STAGES},
            "rss": rss, "disk": disk}


def exponent(history: list[dict], metric: str, stage: str | None = None) -> float:
    result = MIN_EXPONENT[metric]
    for left, right in zip(history, history[1:]):
        ratio = right["size"] / left["size"]
        if ratio < 1.5:
            continue  # Adjacent boundary points amplify ordinary timing noise.
        a, b = (left[metric], right[metric]) if stage is None else (left[metric][stage], right[metric][stage])
        if a <= 0 or b <= 0:
            raise ValueError("positive observed costs are required for projection")
        result = max(result, math.log(b / a) / math.log(ratio) + EXPONENT_MARGIN)
    return result


def project(history: list[dict], target: int, repeats: int) -> dict:
    last = history[-1]
    ratio = target / last["size"]
    wall = {stage: last["wall"][stage] * ratio ** exponent(history, "wall", stage) * SAFETY for stage in STAGES}
    rss = last["rss"] * ratio ** exponent(history, "rss") * SAFETY
    disk = last["disk"] * ratio ** exponent(history, "disk") * SAFETY
    return {"phase_seconds": wall, "cohort_seconds": repeats * sum(wall.values()) + COHORT_FINISH_SECONDS,
            "rss_bytes": math.ceil(rss), "sample_bytes": math.ceil(disk), "cohort_bytes": math.ceil(repeats * disk),
            "exponents": {**{stage: exponent(history, "wall", stage) for stage in STAGES}, "rss": exponent(history, "rss"), "disk": exponent(history, "disk")}}


def admit(plan: dict, profile: str, policy: dict, root: Path, prior_run: str, prior_sha: str) -> tuple[dict, dict]:
    if plan["reviewed_candidate_sha"] != policy["source_sha"] or plan["frozen_candidate_sha"] != policy["source_sha"]:
        raise ValueError("growth requires an explicitly reviewed frozen product source")
    size, predecessor, repeats, phase_seconds, cohort_seconds = TIERS[profile]
    read = lambda path: read_json((root / path).read_text(encoding="utf-8"))
    basis_metadata = read("basis-run.json")
    prior_metadata = read("prior-run.json")
    validate_reuse_run(basis_metadata, policy["boundary_run_id"], policy["boundary_carrier_sha"])
    validate_reuse_run(prior_metadata, prior_run, prior_sha)
    basis_summary, basis_environment = read("basis/boundaries/summary.json"), read("basis/boundaries/environment.json")
    if basis_environment["observer_identity"]["GITHUB_RUN_ID"] != str(basis_metadata["id"]) or basis_environment["observer_identity"]["GITHUB_SHA"] != basis_metadata["head_sha"]:
        raise ValueError("boundary artifact observer differs from its run")
    basis = checked_summary(basis_summary, basis_environment, "boundaries", policy, root / "basis/boundaries")
    if predecessor == "boundaries":
        if prior_run != policy["boundary_run_id"] or prior_sha != policy["boundary_carrier_sha"]:
            raise ValueError("first growth tier must follow the reviewed boundary run")
        history = [observed(basis, 257), observed(basis, 1025)]
    else:
        summary, environment = read(f"prior/{predecessor}/summary.json"), read(f"prior/{predecessor}/environment.json")
        if environment["observer_identity"]["GITHUB_RUN_ID"] != str(prior_metadata["id"]) or environment["observer_identity"]["GITHUB_SHA"] != prior_metadata["head_sha"]:
            raise ValueError("predecessor artifact observer differs from its run")
        samples = checked_summary(summary, environment, predecessor, policy, root / f"prior/{predecessor}")
        previous_review = environment["growth_review"]
        identity = ("source_sha", "source_tree", "binary_sha256", "build_run_id", "build_carrier_sha", "boundary_run_id", "boundary_carrier_sha")
        if previous_review["admitted"] is not True or previous_review["profile"] != predecessor or any(previous_review[key] != policy[key] for key in identity):
            raise ValueError("predecessor growth admission differs")
        history = previous_review["resource_history"] + [observed(samples, TIERS[predecessor][0])]
        if history[:2] != [observed(basis, 257), observed(basis, 1025)]:
            raise ValueError("resource history no longer binds the boundary measurements")
        expected = [257, 1025] + [tier[0] for tier in TIERS.values() if tier[0] < size]
        if [row["size"] for row in history] != expected:
            raise ValueError("resource history must contain every immediate predecessor")
    forecast = project(history, size, repeats)
    limits = {"rss_bytes": RESOURCES["rss_mib"] * 1048576, "sample_bytes": RESOURCES["disk_mib"] * 1048576,
              "cohort_bytes": RESOURCES["cohort_disk_mib"] * 1048576}
    reasons = [f"forecast {name} exceeds its finite budget" for name, maximum in limits.items() if forecast[name] > maximum]
    if max(forecast["phase_seconds"].values()) > phase_seconds:
        reasons.append("forecast phase time exceeds its finite budget")
    if forecast["cohort_seconds"] > cohort_seconds:
        reasons.append("forecast complete repeated cohort exceeds its finite budget")
    # A denial still produces the quantitative review; no pool may be created.
    review = {"schema_version": 1, "reviewed_at_utc": datetime.now(timezone.utc).isoformat(),
              **{key: policy[key] for key in ("source_sha", "source_tree", "binary_sha256", "build_run_id", "build_carrier_sha", "boundary_run_id", "boundary_carrier_sha")},
              "profile": profile, "predecessor_run_id": prior_run, "predecessor_carrier_sha": prior_sha,
              "resource_history": history, "forecast": forecast,
              "forecast_kind": "empirical conservative projection, not a proven resource bound", "safety_factor": SAFETY,
              "admitted": not reasons, "denial_reasons": reasons,
              "coverage": {"timing": True, "full_oracle": True, "counts": False,
                           "counts_not_executed_reason": "higher-tier syscall tracing is outside this bounded timing experiment; retained measured trace coverage ends at N1025"},
              "evidence_sha256": {name: hashlib.sha256((root / name).read_bytes()).hexdigest() for name in ("basis-run.json", "prior-run.json", "basis/boundaries/summary.json", "basis/boundaries/environment.json")}}
    settings = {"baseline_sha": plan["baseline_sha"], "candidate_sha": policy["source_sha"], "sizes": [size], "repeats": repeats,
                "counts_repeats": 0, "counts_sizes": [], "phase_seconds": phase_seconds, "cohort_seconds": cohort_seconds}
    if predecessor != "boundaries":
        for name in (f"prior/{predecessor}/summary.json", f"prior/{predecessor}/environment.json"):
            review["evidence_sha256"][name] = hashlib.sha256((root / name).read_bytes()).hexdigest()
    return settings, review
