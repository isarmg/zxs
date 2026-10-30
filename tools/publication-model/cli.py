#!/usr/bin/env python3
"""有界模型搜索及反例重放；正式执行入口由独立 Actions 调用。"""
import argparse
import json
from pathlib import Path

import model
from search import Limits, replay, search


def emit(value, target):
    rendered = json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n"
    if target:
        Path(target).parent.mkdir(parents=True, exist_ok=True)
        Path(target).write_text(rendered, encoding="utf-8")
    print(rendered, end="")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    explore = commands.add_parser("search", help="deterministic bounded BFS")
    explore.add_argument("--members", type=int, choices=(1, 2, 3, 4), default=1)
    explore.add_argument("--operations", type=int, choices=(1, 2), default=1)
    explore.add_argument("--replicas", type=int, choices=(1, 2), default=1)
    explore.add_argument("--crashes", type=int, choices=(0, 1, 2), default=0)
    explore.add_argument("--fault-budget", type=int, default=0)
    explore.add_argument("--variant", choices=("correct", "shrunken_quorum", "early_ack",
                                               "omit_sync", "timeout_rollback", "lost_binding"),
                         default="correct")
    explore.add_argument("--no-retries", action="store_true")
    explore.add_argument("--max-states", type=int, default=50000)
    explore.add_argument("--max-transitions", type=int, default=500000)
    explore.add_argument("--max-depth", type=int, default=120)
    explore.add_argument("--max-seconds", type=float)
    explore.add_argument("--output")
    explore.add_argument("--trace", help="separate replayable counterexample JSON")
    explore.add_argument("--expect-counterexample", action="store_true")
    rerun = commands.add_parser("replay", help="validate every saved action and state digest")
    rerun.add_argument("input")
    rerun.add_argument("--output")
    args = parser.parse_args(argv)
    if args.command == "replay":
        saved = json.loads(Path(args.input).read_text(encoding="utf-8"))
        trace = saved["trace"] if "trace" in saved else saved
        emit(replay(trace), args.output)
        return 0
    config = model.Config(n=args.members, operations=args.operations, replicas=args.replicas,
                          crashes=args.crashes, fault_budget=args.fault_budget,
                          variant=args.variant, retries=not args.no_retries)
    outcome = search(config, Limits(args.max_states, args.max_transitions,
                                    args.max_depth, args.max_seconds))
    if outcome["trace"] is not None:
        outcome["replay"] = replay(outcome["trace"])
        if args.trace:
            Path(args.trace).parent.mkdir(parents=True, exist_ok=True)
            Path(args.trace).write_text(json.dumps(outcome["trace"], indent=2, sort_keys=True)
                                        + "\n", encoding="utf-8")
    emit(outcome, args.output)
    if args.expect_counterexample:
        return 0 if outcome["status"] == "Counterexample" else 1
    return {"BoundedExhaustive": 0, "Counterexample": 1, "Inconclusive": 2}[outcome["status"]]


if __name__ == "__main__":
    raise SystemExit(main())
