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
    "growth-counts-2048": (2048, "growth-2048", 0, 3600, 10800),
}
RESOURCES = {"rss_mib": 2048, "disk_mib": 4096, "cohort_disk_mib": 8192, "minimum_free_mib": 8192}
COUNTS_RESOURCES = {"rss_mib": 2048, "disk_mib": 12288, "cohort_disk_mib": 24576, "minimum_free_mib": 16384}
SAFETY = 1.5
MIN_EXPONENT = {"wall": 1.25, "rss": 1.0, "disk": 1.0}
EXPONENT_MARGIN = 0.15
COHORT_FINISH_SECONDS = 60


def resources(profile: str) -> dict:
    return COUNTS_RESOURCES if profile == "growth-counts-2048" else RESOURCES


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
        expected = {(size, "counts", 1)} if profile == "growth-counts-2048" else {(size, "timing", repeat) for repeat in range(1, repeats + 1)}
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
            if row["mode"] == "counts" and stage["trace"]["complete"] is not True:
                raise ValueError("predecessor syscall accounting is incomplete")
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
        if metric == "wall" and stage in {"reopen", "recovery"}:
            # Both APIs enter recover_prepared. A usable optional checkpoint
            # changes restore cost discontinuously; budget the cold envelope.
            a = max(left["wall"]["reopen"], left["wall"]["recovery"])
            b = max(right["wall"]["reopen"], right["wall"]["recovery"])
        if a <= 0 or b <= 0:
            raise ValueError("positive observed costs are required for projection")
        result = max(result, math.log(b / a) / math.log(ratio) + EXPONENT_MARGIN)
    return result


def project(history: list[dict], target: int, repeats: int) -> dict:
    last = history[-1]
    ratio = target / last["size"]
    wall = {stage: (max(last["wall"]["reopen"], last["wall"]["recovery"]) if stage in {"reopen", "recovery"} else last["wall"][stage])
            * ratio ** exponent(history, "wall", stage) * SAFETY for stage in STAGES}
    rss = last["rss"] * ratio ** exponent(history, "rss") * SAFETY
    disk = last["disk"] * ratio ** exponent(history, "disk") * SAFETY
    return {"phase_seconds": wall, "cohort_seconds": repeats * sum(wall.values()) + COHORT_FINISH_SECONDS,
            "rss_bytes": math.ceil(rss), "sample_bytes": math.ceil(disk), "cohort_bytes": math.ceil(repeats * disk),
            "exponents": {**{stage: exponent(history, "wall", stage) for stage in STAGES}, "rss": exponent(history, "rss"), "disk": exponent(history, "disk")}}


def io_observation(root: Path, sample: dict, stage: str) -> dict:
    directory = root / "samples" / f"{sample['operations']:06d}-{sample['mode']}-{sample['repeat']:02d}-candidate"
    syscr = 0
    with (directory / f"{stage}.process-samples.jsonl").open(encoding="utf-8") as rows:
        for raw in rows:
            for process in read_json(raw)["processes"]:
                if process["io"]:
                    syscr = max(syscr, process["io"]["syscr"])
    rchar = sample["stages"][stage]["driver"]["telemetry"][-1]["rchar"]
    if syscr <= 0 or rchar <= 0:
        raise ValueError("positive sampled syscall and driver logical-read evidence is required")
    return {"sampled_syscr_peak": syscr, "rchar": rchar}


def counts_projection(basis: list[dict], target: list[dict], basis_root: Path, target_root: Path) -> dict:
    native = next(row for row in basis if row["operations"] == 1025 and row["mode"] == "timing")
    traced = next(row for row in basis if row["operations"] == 1025 and row["mode"] == "counts")
    current = next(row for row in target if row["mode"] == "timing")
    wall, evidence = {}, {}
    trace_bytes = 0
    for stage in STAGES:
        reference = "build" if stage == "build" else "reopen"  # Cold replay, never the old checkpoint restore.
        old_io = io_observation(basis_root, native, reference)
        new_io = io_observation(target_root, current, stage)
        ratio = max(current["operations"] / 1025, new_io["sampled_syscr_peak"] / old_io["sampled_syscr_peak"], new_io["rchar"] / old_io["rchar"])
        trace = basis_root / "samples/001025-counts-01-candidate" / f"{reference}.strace.txt"
        original_bytes = trace.stat().st_size
        if original_bytes <= 0:
            raise ValueError("positive complete baseline trace is required")
        trace_bytes += original_bytes * ratio * SAFETY
        native_wall = native["stages"][reference]["supervisor"]["wall_seconds"]
        trace_wall = traced["stages"][reference]["supervisor"]["wall_seconds"]
        wall[stage] = current["stages"][stage]["supervisor"]["wall_seconds"] + max(0.0, trace_wall - native_wall) * ratio * SAFETY
        evidence[stage] = {"cold_reference_stage": reference, "basis": old_io, "target": new_io,
                           "io_growth_factor": ratio, "basis_trace_bytes": original_bytes}
    measured = observed(target, current["operations"])
    # Include one whole native pool/oracle plus raw trace and three hours of
    # observer logs. Parse/compression is streamed and receives 10 minutes.
    disk = math.ceil(trace_bytes + measured["disk"] + 512 * 1048576)
    return {"phase_seconds": wall, "cohort_seconds": sum(wall.values()) + 600,
            "rss_bytes": math.ceil(measured["rss"] * SAFETY), "sample_bytes": disk, "cohort_bytes": disk,
            "io_projection": evidence, "trace_bytes": math.ceil(trace_bytes), "finish_seconds": 600}


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
        expected = [257, 1025] + [tier[0] for key, tier in TIERS.items() if key != "growth-counts-2048" and (tier[0] < size or (profile == "growth-counts-2048" and tier[0] == size))]
        if [row["size"] for row in history] != expected:
            raise ValueError("resource history must contain every immediate predecessor")
    count_only = profile == "growth-counts-2048"
    forecast = counts_projection(basis, samples, root / "basis/boundaries", root / f"prior/{predecessor}") if count_only else project(history, size, repeats)
    budget = resources(profile)
    limits = {"rss_bytes": budget["rss_mib"] * 1048576, "sample_bytes": budget["disk_mib"] * 1048576,
              "cohort_bytes": budget["cohort_disk_mib"] * 1048576}
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
              "admission_time_envelope": "max(reopen, recovery); raw observations remain unchanged; optional checkpoint eligibility can change",
              "admitted": not reasons, "denial_reasons": reasons,
              "budgets": {**budget, "phase_seconds": phase_seconds, "cohort_seconds": cohort_seconds},
              "coverage": ({"timing": False, "full_oracle": True, "counts": True, "timing_counterpart_run_id": prior_run}
                           if count_only else {"timing": True, "full_oracle": True, "counts": False,
                           "counts_not_executed_reason": "this run is explicitly untraced; higher-tier count coverage requires its separate admitted run"}),
              "evidence_sha256": {name: hashlib.sha256((root / name).read_bytes()).hexdigest() for name in ("basis-run.json", "prior-run.json", "basis/boundaries/summary.json", "basis/boundaries/environment.json")}}
    settings = {"baseline_sha": plan["baseline_sha"], "candidate_sha": policy["source_sha"], "sizes": [size], "repeats": repeats,
                "counts_repeats": 1 if count_only else 0, "counts_sizes": [size] if count_only else [],
                "phase_seconds": phase_seconds, "cohort_seconds": cohort_seconds}
    if predecessor != "boundaries":
        for name in (f"prior/{predecessor}/summary.json", f"prior/{predecessor}/environment.json"):
            review["evidence_sha256"][name] = hashlib.sha256((root / name).read_bytes()).hexdigest()
    return settings, review
