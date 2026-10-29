"""GitHub-only, explicitly selected-prefix suffix coverage, separate from root BFS."""
import argparse
from dataclasses import asdict
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/publication-model"))

from deep_search import explore_suffix
from model import Config
from search import Limits, replay


def member(actions, phase, index):
    actions.extend(f"{phase}:{index}:{step}" for step in
                   ("write", "file_sync", "install", "directory_sync"))


def ready(config):
    actions = []
    for phase, count in (("CONTENT", config.replicas), ("PREPARE", config.n)):
        for index in range(count):
            member(actions, phase, index)
        actions.append("barrier:" + phase)
    actions.append("issue_accept")
    return actions


def accepted(config, members):
    actions = ready(config)
    for index in members:
        member(actions, "ACCEPT", index)
    return actions


def one_publication(config):
    actions = accepted(config, range(config.n))
    actions.append("barrier:ACCEPT")
    for phase, count in (("FINALIZE", config.n), ("COMMIT_PROOF", config.replicas)):
        for index in range(count):
            member(actions, phase, index)
        actions.append("barrier:" + phase)
    member(actions, "PUBLICATION", config.n - 1)
    return actions


def ack(config):
    actions = one_publication(config)
    for index in range(config.n - 1):
        member(actions, "PUBLICATION", index)
    actions.append("barrier:PUBLICATION")
    for index in range(config.replicas):
        member(actions, "PUBLICATION_PROOF", index)
    actions.extend(("barrier:PUBLICATION_PROOF", "publish:atomic_state_receipt_event"))
    return actions


def write(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    config2 = Config(n=2, crashes=2, fault_budget=1)
    config3 = Config(n=3, replicas=2, crashes=2, fault_budget=1)
    config4 = Config(n=4, replicas=2, crashes=2, fault_budget=1)
    recover3 = Config(n=3, crashes=2, fault_budget=1)
    preserve = accepted(recover3, (0,)) + ["crash:process", "observe:0:1",
                "observe:1:0", "recover:preserve_candidate"]
    abort = ready(recover3) + ["crash:process", "observe:0:0", "observe:2:0",
                            "recover:abort_evidence", "RECOVER_ABORT:2:write"]
    baseline = Config(n=2)
    scenarios = [
        ("n2-published-client-suffix", baseline, ack(baseline)),
        ("n2-ready-accept", config2, ready(config2)),
        ("n3-nonprefix-accept", config3, accepted(config3, (2, 0))),
        ("n4-nonprefix-accept", config4, accepted(config4, (2, 0))),
        ("n3-recovery-preserve", recover3, preserve),
        ("n3-abort-resolution-write", recover3, abort),
        ("n3-single-publication", config3, one_publication(config3)),
    ]
    records = []
    failed = False
    for name, config, actions in scenarios:
        result = explore_suffix(config, actions, Limits(max_states=20000,
                   max_transitions=200000, max_depth=20, max_seconds=15))
        write(args.output / (name + ".json"), result)
        write(args.output / ("prefix-" + name + ".json"), result["prefix"]["trace"])
        checked = replay(result["prefix"]["trace"])
        write(args.output / ("prefix-replay-" + name + ".json"), checked)
        assert checked["valid"] and not checked["counterexample"]
        record = {"name": name, "config": asdict(config), "scope": result["scope"],
                  "status": result["status"], "truncation": result["truncation"],
                  "prefix_length": result["prefix"]["length"],
                  "prefix_requested_length": result["prefix"]["requested_length"],
                  "prefix_complete": result["prefix"]["length"] == result["prefix"]["requested_length"],
                  "counts": result["counts"], "coverage": result["coverage"]}
        records.append(record)
        if name == "n2-published-client-suffix":
            failed = failed or result["status"] != "LocalExhaustive"
        if result["status"] == "Counterexample":
            failed = True
            write(args.output / ("counterexample-" + name + ".json"), result["trace"])
            write(args.output / ("replay-" + name + ".json"), replay(result["trace"]))
        print(json.dumps(record, sort_keys=True), flush=True)
    mutant = Config(n=3, variant="early_ack")
    found = explore_suffix(mutant, accepted(mutant, (2, 0)),
              Limits(max_states=1000, max_transitions=10000, max_depth=8, max_seconds=10))
    write(args.output / "mutation-early_ack.json", found)
    mutation = {"status": found["status"], "scope": found["scope"]}
    if found["status"] == "Counterexample":
        write(args.output / "counterexample-early_ack.json", found["trace"])
        rerun = replay(found["trace"])
        write(args.output / "replay-early_ack.json", rerun)
        mutation["replayed"] = rerun["valid"] and rerun["counterexample"]
        failed = failed or not mutation["replayed"]
    else:
        failed = True
    summary = {"schema": "zxs-publication-model-deep-validation-v1",
               "scope": "selected-prefix-suffix, never root exhaustiveness",
               "tool_commit": subprocess.check_output(["git", "rev-parse", "HEAD"],
                                                         cwd=ROOT, text=True).strip(),
               "normal": records, "mutation": mutation, "validation_failed": failed,
               "claim": "Only a validated selected prefix and its explicitly bounded suffix; "
                        "no successor pruning or fault/crash budget reset. Inconclusive is not a pass."}
    write(args.output / "summary.json", summary)
    print(json.dumps(mutation, sort_keys=True), flush=True)
    return int(failed)


if __name__ == "__main__":
    raise SystemExit(main())
