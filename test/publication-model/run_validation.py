"""GitHub-only bounded validation matrix; raw individual results stay intact."""
import argparse
import json
from pathlib import Path
import platform
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/publication-model"))

from model import Config
from search import Limits, replay, search


def write(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def revision():
    return subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, check=True,
                          text=True, capture_output=True).stdout.strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    environment = {"python": sys.version, "platform": platform.platform(),
                   "tool_commit": revision(),
                   "spec_commit": "e2f33d9882cf7263412d9370b6a703e0b216357d",
                   "arguments": sys.argv}
    write(args.output / "environment.json", environment)
    matrix = [
        ("n1-one-slot", dict(n=1), 50000, 30),
        ("n1-two-slots", dict(n=1, operations=2), 50000, 30),
        ("n2-one-copy", dict(n=2), 50000, 30),
        ("n2-two-copies", dict(n=2, replicas=2), 50000, 30),
        ("n3-full-publication", dict(n=3, replicas=2), 150000, 45),
        ("n4-full-publication", dict(n=4, replicas=2), 250000, 60),
        ("n1-one-crash", dict(n=1, crashes=1), 35000, 25),
        ("n1-recovery-crash", dict(n=1, crashes=2), 35000, 25),
        ("n2-failure-recovery", dict(n=2, crashes=1, fault_budget=1), 25000, 25),
        ("n3-nonprefix-failures", dict(n=3, fault_budget=3), 25000, 25),
        ("n3-recovery", dict(n=3, replicas=2, crashes=2, fault_budget=2), 25000, 25),
        ("n3-two-slots", dict(n=3, operations=2, crashes=2, fault_budget=1), 25000, 25),
        ("n4-nonprefix-failures", dict(n=4, fault_budget=4), 25000, 25),
        ("n4-recovery", dict(n=4, replicas=2, crashes=2, fault_budget=2), 25000, 25),
        ("n4-two-slots", dict(n=4, operations=2, crashes=2, fault_budget=1), 25000, 25),
    ]
    records = []
    failed = False
    for name, options, states, seconds in matrix:
        result = search(Config(**options), Limits(max_states=states,
                        max_transitions=states * 12, max_depth=220, max_seconds=seconds))
        write(args.output / (name + ".json"), result)
        record = {"name": name, "config": result["config"], "status": result["status"],
                  "truncation": result["truncation"], "counts": result["counts"]}
        records.append(record)
        print(json.dumps(record, sort_keys=True), flush=True)
        if name in ("n1-one-slot", "n1-two-slots", "n2-one-copy", "n2-two-copies"):
            failed = failed or result["status"] != "BoundedExhaustive"
        if result["status"] == "Counterexample":
            failed = True
            write(args.output / (name + "-counterexample.json"), result["trace"])
            write(args.output / (name + "-replay.json"), replay(result["trace"]))
    variants = [
        ("shrunken_quorum", dict(n=2, fault_budget=1)),
        ("early_ack", dict(n=1)),
        ("omit_sync", dict(n=1)),
        ("timeout_rollback", dict(n=1)),
        ("lost_binding", dict(n=1)),
    ]
    mutations = []
    for variant, options in variants:
        result = search(Config(variant=variant, **options), Limits(max_states=80000,
                        max_transitions=900000, max_depth=220, max_seconds=45))
        write(args.output / ("mutation-" + variant + ".json"), result)
        record = {"variant": variant, "status": result["status"], "counts": result["counts"]}
        if result["status"] == "Counterexample":
            write(args.output / ("counterexample-" + variant + ".json"), result["trace"])
            replayed = replay(result["trace"])
            write(args.output / ("replay-" + variant + ".json"), replayed)
            record["replayed"] = replayed["valid"] and replayed["counterexample"]
            record["violations"] = replayed["violations"]
            failed = failed or not record["replayed"]
        else:
            failed = True
        mutations.append(record)
        print(json.dumps(record, sort_keys=True), flush=True)
    write(args.output / "summary.json", {"environment": environment, "normal": records,
          "mutations": mutations, "validation_failed": failed,
          "claim": "Only BoundedExhaustive rows completed the declared finite search; "
                   "Inconclusive rows are not passes. Mutants are intentionally incorrect."})
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
