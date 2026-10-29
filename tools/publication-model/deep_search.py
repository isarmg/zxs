"""Explore every enabled suffix after one explicitly validated model prefix.

LocalExhaustive is conditional on that selected prefix. It is never a root graph
proof. Resource or depth frontiers remain Inconclusive; no actions are pruned.
"""
from collections import deque
from dataclasses import asdict
import time

import model
import properties
from search import Limits, _ordered_successors, state_digest


def explore_suffix(config, actions, limits=None, *, max_prefix_steps=512,
                   max_prefix_successors=100000):
    limits = limits or Limits()
    if max_prefix_steps < 0 or max_prefix_successors < 1:
        raise ValueError("invalid prefix resource budget")
    if not isinstance(actions, (list, tuple)):
        raise ValueError("prefix actions must be a finite list or tuple")
    started = time.monotonic()
    root = state = model.initial(config)
    initial_violations = properties.check(None, state, config)
    if initial_violations:
        raise ValueError(f"unsafe initial state: {initial_violations}")
    prefix_trace = {"schema": "zxs-publication-model-trace-v1", "config": asdict(config),
                    "initial_digest": state_digest(root), "steps": []}
    prefix_generated = 0
    nodes, parents, depths = [], [], []
    queue = deque()
    transitions = expanded = max_depth = 0
    phases = set()

    def expired():
        return limits.max_seconds is not None and time.monotonic() - started >= limits.max_seconds

    def result(status, reason=None, trace=None, violations=()):
        return {"schema": "zxs-publication-model-suffix-v1",
                "scope": "selected-prefix-suffix", "config": asdict(config),
                "limits": asdict(limits), "status": status, "truncation": reason,
                "prefix": {"requested_length": len(actions), "length": len(prefix_trace["steps"]),
                           "source": "initial(config) followed by legal, property-checked actions",
                           "trace": prefix_trace, "end_digest": state_digest(state),
                           "generated_successors": prefix_generated,
                           "max_steps": max_prefix_steps,
                           "max_successors_per_step": max_prefix_successors,
                           "crashes_consumed": state.crash_count,
                           "faults_consumed": state.fault_count},
                "counts": {"states": len(nodes), "transitions": transitions,
                           "expanded": expanded, "max_depth": max_depth, "queued": len(queue)},
                "coverage": {"suffix_phases_checked": sorted(phases),
                             "max_full_trace_depth": len(prefix_trace["steps"]) + max_depth},
                "elapsed_seconds": time.monotonic() - started,
                "violations": list(violations), "trace": trace}

    for number, requested in enumerate(actions):
        if number >= max_prefix_steps:
            return result("Inconclusive", "max_prefix_steps")
        target = None
        for generated, (action, candidate) in enumerate(_ordered_successors(state, config), 1):
            if expired():
                return result("Inconclusive", "max_seconds_during_prefix")
            if generated > max_prefix_successors:
                return result("Inconclusive", "max_prefix_successors")
            prefix_generated += 1
            if action == requested:
                target = candidate
        if target is None:
            raise ValueError(f"prefix step {number + 1}: action is not enabled: {requested}")
        violations = properties.check(state, target, config)
        if violations:
            raise ValueError(f"unsafe prefix step {number + 1} ({requested}): {violations}")
        prefix_trace["steps"].append({"action": requested, "state_digest": state_digest(target)})
        state = target

    seed = state
    nodes, parents, depths = [seed], [None], [0]
    seen, queue = {seed: 0}, deque([0])
    phases.add(seed.phase)
    depth_frontier = False

    def witness(index, final):
        suffix_steps = [{"action": final[0], "state_digest": state_digest(final[1])}]
        while parents[index] is not None:
            previous, action = parents[index]
            suffix_steps.append({"action": action, "state_digest": state_digest(nodes[index])})
            index = previous
        suffix_steps.reverse()
        return {"schema": prefix_trace["schema"], "config": prefix_trace["config"],
                "initial_digest": prefix_trace["initial_digest"],
                "steps": list(prefix_trace["steps"]) + suffix_steps}

    while queue:
        if expired():
            return result("Inconclusive", "max_seconds")
        index = queue.popleft()
        current, depth = nodes[index], depths[index]
        expanded += 1
        for action, following in _ordered_successors(current, config):
            if expired():
                return result("Inconclusive", "max_seconds")
            if transitions >= limits.max_transitions:
                return result("Inconclusive", "max_transitions")
            transitions += 1
            if depth >= limits.max_depth:
                depth_frontier = True
                continue
            max_depth = max(max_depth, depth + 1)
            phases.add(following.phase)
            violations = properties.check(current, following, config)
            if violations:
                trace = witness(index, (action, following))
                trace["violations"] = list(violations)
                return result("Counterexample", trace=trace, violations=violations)
            if following in seen:
                continue
            if len(nodes) >= limits.max_states:
                return result("Inconclusive", "max_states")
            seen[following] = len(nodes)
            queue.append(len(nodes))
            nodes.append(following)
            parents.append((index, action))
            depths.append(depth + 1)
    return result("Inconclusive", "max_depth") if depth_frontier else result("LocalExhaustive")
