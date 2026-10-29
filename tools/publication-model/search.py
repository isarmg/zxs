"""确定性 BFS、资源边界和经过摘要核对的反例重放。

Only model transitions generate states. This module never calls the product or
uses a recovery implementation as an oracle. Bounds are search resources, not
assumptions that silently make an unfinished run pass.
"""
from collections import deque
from dataclasses import asdict, dataclass
import hashlib
import json
import time

import model
import properties


@dataclass(frozen=True)
class Limits:
    max_states: int = 50000
    max_transitions: int = 500000
    max_depth: int = 120
    max_seconds: float | None = None

    def __post_init__(self):
        if min(self.max_states, self.max_transitions) < 1 or self.max_depth < 0:
            raise ValueError("state/transition budgets must be positive; depth nonnegative")
        if self.max_seconds is not None and self.max_seconds <= 0:
            raise ValueError("time budget must be positive")


def state_digest(state):
    wire = json.dumps(model.state_to_dict(state), sort_keys=True,
                      separators=(",", ":"), ensure_ascii=True).encode()
    return hashlib.sha256(wire).hexdigest()


def _ordered_successors(state, config):
    # The model defines a deterministic generation order. Streaming matters:
    # crash byte/name outcomes can be much wider than the global search budget.
    # Sorting/materializing all of them would defeat that budget before search.
    actions = set()
    for action, following in model.successors(state, config):
        if action in actions:
            raise ValueError("model generated ambiguous duplicate action labels")
        actions.add(action)
        yield action, following


def _trace(config, nodes, parents, index, final=None):
    path = []
    while parents[index] is not None:
        predecessor, action = parents[index]
        path.append({"action": action, "state_digest": state_digest(nodes[index])})
        index = predecessor
    path.reverse()
    if final is not None:
        action, state = final
        path.append({"action": action, "state_digest": state_digest(state)})
    return {"schema": "zxs-publication-model-trace-v1", "config": asdict(config),
            "initial_digest": state_digest(nodes[0]), "steps": path}


def search(config, limits=None):
    """Breadth-first exploration with shortest discovered violation witnesses.

    All generated edges, including edges to previously seen states, are checked.
    Exact immutable states are dictionary keys; digest collisions do not dedupe.
    Every unexamined edge at the depth frontier makes the run inconclusive,
    including backedges, because some properties concern transitions.
    """
    limits = limits or Limits()
    started = time.monotonic()
    first = model.initial(config)
    nodes, parents, depths = [first], [None], [0]
    seen, queue = {first: 0}, deque([0])
    transitions = expanded = max_depth = 0
    depth_frontier = False
    violations = properties.check(None, first, config)

    def result(status, reason=None, trace=None, found=None):
        return {"schema": "zxs-publication-model-search-v1", "config": asdict(config),
                "limits": asdict(limits), "status": status, "truncation": reason,
                "counts": {"states": len(nodes), "transitions": transitions,
                           "expanded": expanded, "max_depth": max_depth,
                           "queued": len(queue)},
                "elapsed_seconds": time.monotonic() - started,
                "violations": list(found or ()), "trace": trace,
                "scope": "declared finite abstraction only"}

    if violations:
        return result("Counterexample", trace=_trace(config, nodes, parents, 0),
                      found=violations)
    while queue:
        if limits.max_seconds is not None and time.monotonic() - started >= limits.max_seconds:
            return result("Inconclusive", "max_seconds")
        index = queue.popleft()
        state, depth = nodes[index], depths[index]
        expanded += 1
        for action, following in _ordered_successors(state, config):
            if limits.max_seconds is not None and time.monotonic() - started >= limits.max_seconds:
                return result("Inconclusive", "max_seconds")
            if transitions >= limits.max_transitions:
                return result("Inconclusive", "max_transitions")
            transitions += 1
            if depth >= limits.max_depth:
                depth_frontier = True
                continue
            violations = properties.check(state, following, config)
            if violations:
                witness = _trace(config, nodes, parents, index, (action, following))
                witness["violations"] = list(violations)
                return result("Counterexample", trace=witness, found=violations)
            if following in seen:
                continue
            if len(nodes) >= limits.max_states:
                return result("Inconclusive", "max_states")
            seen[following] = len(nodes)
            queue.append(len(nodes))
            nodes.append(following)
            parents.append((index, action))
            depths.append(depth + 1)
            max_depth = max(max_depth, depth + 1)
    if depth_frontier:
        return result("Inconclusive", "max_depth")
    return result("BoundedExhaustive")


def replay(trace, max_successors=1000000):
    """Reconstruct every transition and require exact stored state digests.

    Malformed traces, missing/ambiguous transitions and changed digests are errors,
    never a successful replay. A valid non-violating trace is explicitly distinct
    from a replayed counterexample.
    """
    if max_successors < 1:
        raise ValueError("replay successor budget must be positive")
    if trace["schema"] != "zxs-publication-model-trace-v1":
        raise ValueError("unsupported trace schema")
    config = model.Config(**trace["config"])
    state = model.initial(config)
    if state_digest(state) != trace["initial_digest"]:
        raise ValueError("initial state digest mismatch")
    violations = properties.check(None, state, config)
    first_violation_step = 0 if violations else None
    for number, entry in enumerate(trace["steps"], 1):
        following = None
        for count, (action, candidate) in enumerate(_ordered_successors(state, config), 1):
            if count > max_successors:
                raise ValueError(f"step {number}: replay successor budget exceeded")
            if action == entry["action"]:
                following = candidate
        if following is None:
            raise ValueError(f"step {number}: action is not enabled: {entry['action']}")
        if state_digest(following) != entry["state_digest"]:
            raise ValueError(f"step {number}: state digest mismatch")
        found = properties.check(state, following, config)
        if found and first_violation_step is None:
            first_violation_step = number
        violations = found or violations
        state = following
    expected = trace.get("violations")
    if expected is not None and list(violations) != expected:
        raise ValueError("replayed violation differs from recorded counterexample")
    return {"schema": "zxs-publication-model-replay-v1", "valid": True,
            "counterexample": bool(violations), "violations": list(violations),
            "steps": len(trace["steps"]), "first_violation_step": first_violation_step,
            "final_digest": state_digest(state), "config": asdict(config)}
