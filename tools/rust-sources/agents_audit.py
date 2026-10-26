#!/usr/bin/env python3
"""Read-only candidate inventory, not a semantic AGENTS.md compliance verdict."""
import argparse
import collections
import hashlib
import json
from pathlib import Path
import re

PATTERNS = {
    "ignored_result_syntax": r"\blet\s+_\s*=(?![=>])|(?<!\w)_\s*=(?![=>])|\.ok\(\)|\bif\s+let\s+Ok\(",
    "default_fallback_syntax": r"\.unwrap_or_default\(\)|\.unwrap_or\(|\.unwrap_or_else\(",
    "erased_error_candidate": r"map_err\(\|_\||map_err\(\|[^|]*\|[^\n]*(?:to_string\(|format!\()",
    "wildcard_noop_candidate": r"_\s*=>\s*\{\s*\}|_\s*=>\s*unreachable!",
    "boundary_or_internal_arithmetic": r"\bchecked_\w+\(|\btry_from\(",
    "ownership_review_candidate": r"\.clone\(\)",
    "panic_or_retry_boundary_candidate": r"\bcatch_unwind\(|\btimeout\(|\btry_lock\(",
}


def inventory(root):
    ledger = json.loads((root / "compliance/rust-sources.json").read_text())
    origins = json.loads((root / "compliance/provenance/origins.json").read_text())
    original_hashes = collections.defaultdict(list)
    original_packages = {}
    for package in origins["packages"]:
        original_packages[package["origin_directory"]] = package
        for path, digest in package["original_files_sha256"].items():
            if path.endswith(".rs"):
                original_hashes[digest].append({"identity": package["identity"], "path": path})
    packages = sorted(ledger["packages"].items(), key=lambda item: len(item[0]), reverse=True)
    records = []
    counts = collections.Counter()
    provenance_counts = collections.Counter()
    for path in sorted((root / "src/foundation").rglob("*.rs")):
        relative = path.relative_to(root).as_posix()
        data = path.read_bytes()
        digest = hashlib.sha256(data).hexdigest()
        owner = next(((directory, package) for directory, package in packages
                      if relative.startswith(directory + "/")), None)
        same_path = False
        if owner:
            directory, package = owner
            origin = original_packages.get(package["origin_directory"], {})
            same_path = origin.get("original_files_sha256", {}).get(relative[len(directory) + 1:]) == digest
        status = ("original_same_path_bytes" if same_path else
                  "original_relocated_bytes" if original_hashes[digest] else
                  "changed_or_added_requires_mapping_review")
        provenance_counts[status] += 1
        candidates = []
        for number, line in enumerate(data.decode("utf-8").splitlines(), 1):
            for rule, pattern in PATTERNS.items():
                if re.search(pattern, line):
                    counts[rule] += 1
                    candidates.append({"line": number, "rule": rule, "text": line.strip(),
                                       "review": "unreviewed_syntax_not_violation"})
        records.append({"path": relative, "sha256": digest,
                        "origin_directory": owner[1]["origin_directory"] if owner else None,
                        "provenance": status, "identical_original_files": original_hashes[digest],
                        "semantic_review": "not_line_by_line_reviewed", "candidates": candidates})
    return {"schema": 1, "scope": "all current src/foundation/**/*.rs; no compilation",
            "limitations": ["Regex includes comments, docs, tests and legitimate public boundaries.",
                             "Exact bytes prove provenance, not semantic compliance.",
                             "Changed/added includes merges and mechanical naming changes.",
                             "No candidates does not imply compliance; this omits many rules."],
            "review_required": ["Trace input and caller; retain external, public API and FFI checks.",
                                "Distinguish cancellation, Drop and infallible formatting from I/O failure.",
                                "Classify confirmed violation / required boundary / valid contract / unresolved.",
                                "Record evidence, minimal fix and regression test for each reviewed candidate."],
            "summary": {"files": len(records), "provenance": dict(provenance_counts),
                        "candidate_lines_by_rule": dict(counts)}, "files": records}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[2])
    args = parser.parse_args()
    print(json.dumps(inventory(args.root.resolve()), ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
