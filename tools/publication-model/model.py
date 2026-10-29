"""Finite publication protocol transitions; no production code is imported.

The safety oracle lives in properties.py.  Artifact fields describe separate
filesystem observations, rather than a single durable flag. See SPEC.md.
"""
from dataclasses import asdict, dataclass, replace
from itertools import product
from typing import Iterator

CONTENT, PREPARE, ACCEPT, FINALIZE, COMMIT_PROOF, PUBLICATION, PUBLICATION_PROOF = range(7)
RECOVERY_RESOLUTION = 7
KINDS = ("content", "prepare", "accept", "finalize", "commit_proof", "publication", "publication_proof", "recovery_resolution")
STAGES = tuple(name.upper() for name in KINDS[:7])
ABSENT, FULL, TRUNCATED, CORRUPT = range(4)
VARIANTS = ("correct", "shrunken_quorum", "early_ack", "omit_sync", "timeout_rollback", "lost_binding")


@dataclass(frozen=True)
class Config:
    n: int = 1
    replicas: int = 1
    operations: int = 1
    crashes: int = 0
    fault_budget: int = 0
    variant: str = "correct"
    retries: bool = True

    def __post_init__(self):
        if self.n not in (1, 2, 3, 4):
            raise ValueError("n must be 1..4")
        if not 1 <= self.replicas <= min(2, self.n):
            raise ValueError("replicas must be 1..min(2,n)")
        if self.operations not in (1, 2) or not 0 <= self.crashes <= 2:
            raise ValueError("operations=1..2; crashes=0..2")
        if self.fault_budget < 0 or self.variant not in VARIANTS:
            raise ValueError("invalid fault budget or variant")


@dataclass(frozen=True)
class Artifact:
    data: int = ABSENT
    stable_data: int = ABSENT
    file_synced: bool = False
    entry: bool = False
    dir_synced: bool = False
    done: bool = False
    failed: bool = False


@dataclass(frozen=True)
class State:
    slot: int
    phase: str
    artifacts: tuple
    online: tuple
    read_fault: tuple
    crash_count: int
    fault_count: int
    issued: tuple
    decision_witness: tuple
    known_publication: tuple
    results: tuple
    state_versions: tuple
    events: tuple
    publish_count: tuple
    publication_witness: tuple
    abort_witness: tuple
    publication_origin: tuple
    publication_result: tuple
    abort_resolution: tuple
    observations: tuple
    recovery_mode: bool
    recovery_result: int
    normal_publication_seen: bool
    reply_lost: bool
    retry_count: int
    binding: tuple
    index_failed: bool
    response: str
    recovery_round: int


def initial(config: Config) -> State:
    zero = (0,) * config.operations
    rows = tuple(tuple(tuple(Artifact() for _ in KINDS) for _ in range(config.n))
                 for _ in range(config.operations))
    return State(0, "CONTENT", rows, (True,) * config.n, (False,) * config.n,
                 0, 0, (False,) * config.operations, zero, zero, zero, (), (),
                 zero, ((),) * config.operations, zero, ((),) * config.operations, zero, zero, (-1,) * config.n,
                 False, 0, False, False, 0, zero, False, "none", 0)


def _set(values: tuple, index: int, value):
    return values[:index] + (value,) + values[index + 1:]


def _durable(a: Artifact) -> bool:
    return a.stable_data == FULL and a.dir_synced


def _put(state: State, member: int, kind: int, value: Artifact) -> State:
    row = _set(state.artifacts[state.slot][member], kind, value)
    members = _set(state.artifacts[state.slot], member, row)
    artifacts = _set(state.artifacts, state.slot, members)
    result = replace(state, artifacts=artifacts)
    if kind == PUBLICATION and value.data == FULL:
        result = replace(result, publication_result=_set(
            result.publication_result, result.slot, result.recovery_result or 1))
        if not result.publication_origin[result.slot]:
            result = replace(result, publication_origin=_set(
                result.publication_origin, result.slot, _certificate(result)))
        if value.entry and result.publication_result[result.slot] == 1:
            result = replace(result, normal_publication_seen=True)
        if _durable(value):
            result = replace(result, known_publication=_set(
                result.known_publication, result.slot, result.recovery_result or 1))
    return _decision(result)


def _decision(state: State) -> State:
    mask = sum(1 << m for m, row in enumerate(state.artifacts[state.slot])
               if _durable(row[ACCEPT]))
    q = len(state.online) // 2 + 1
    if mask.bit_count() >= q and not state.decision_witness[state.slot]:
        return replace(state, decision_witness=_set(state.decision_witness, state.slot, mask))
    return state


def _targets(config: Config, kind: int):
    return range(config.replicas if kind in (CONTENT, COMMIT_PROOF, PUBLICATION_PROOF) else config.n)


def _certificate(state: State):
    return tuple(sum(1 << k for k, a in enumerate(row) if _durable(a))
                 for row in state.artifacts[state.slot])


def _source_available(state: State, config: Config):
    return any(state.online[m] and not state.read_fault[m] and
               state.artifacts[state.slot][m][CONTENT].entry and
               state.artifacts[state.slot][m][CONTENT].data == FULL
               for m in range(config.replicas))


def _healthy_content(state: State, config: Config):
    return all(state.artifacts[state.slot][m][CONTENT].entry and
               state.artifacts[state.slot][m][CONTENT].data == FULL and
               _durable(state.artifacts[state.slot][m][CONTENT])
               for m in range(config.replicas))


def _publication_certificate(state: State):
    current = _certificate(state)
    origin = state.publication_origin[state.slot]
    if not origin:
        return current
    # PUBLICATION carries the prior protected-content receipts. Later content
    # damage changes health, but cannot erase that historical fact.
    historical_bits = (1 << CONTENT) | (1 << COMMIT_PROOF)
    return tuple(mask | (origin[m] & historical_bits) for m, mask in enumerate(current))


def _apply(state: State, result: int, duplicate: bool = False) -> State:
    slot = state.slot
    first = not state.results[slot]
    additions = (slot,) if first or duplicate else ()
    return replace(state, results=_set(state.results, slot, result),
                   state_versions=state.state_versions + additions,
                   events=state.events + additions,
                   publish_count=_set(state.publish_count, slot,
                                      state.publish_count[slot] + bool(additions)),
                   publication_witness=_set(state.publication_witness, slot,
                                           _publication_certificate(state) if first else state.publication_witness[slot]),
                   binding=_set(state.binding, slot, slot + 1), phase="ACK",
                   response="none")


def _begin_recovery(state: State) -> State:
    return replace(state, phase="RECOVER_OBSERVE", observations=(-1,) * len(state.online),
                   recovery_mode=True, recovery_result=0, normal_publication_seen=False, response="pending",
                   recovery_round=min(3, state.recovery_round + 1))


def _io_success(a: Artifact):
    if a.failed:
        return None
    if a.data != FULL:
        return "write", replace(a, data=FULL)
    if not a.file_synced:
        return "file_sync", replace(a, stable_data=FULL, file_synced=True)
    if not a.entry:
        return "install", replace(a, entry=True)
    if not a.dir_synced:
        return "directory_sync", replace(a, dir_synced=True, done=True)
    if not a.done:
        return "confirm", replace(a, done=True)
    return None


def _io_failures(a: Artifact, step: str):
    """A syscall error does not imply its side effects were absent."""
    if step == "write":
        for value in (ABSENT, TRUNCATED, CORRUPT, FULL):
            yield str(value), replace(a, data=value, failed=True)
    elif step == "file_sync":
        for value in dict.fromkeys((a.stable_data, FULL)):
            yield str(value), replace(a, stable_data=value, failed=True)
    elif step == "install":
        for visible in (False, True):
            yield str(int(visible)), replace(a, entry=visible, failed=True)
    elif step == "directory_sync":
        for stable in (False, True):
            yield str(int(stable)), replace(a, dir_synced=stable, failed=True)


def _stage_steps(state: State, config: Config) -> Iterator[tuple[str, State]]:
    kind = RECOVERY_RESOLUTION if state.phase == "RECOVER_ABORT" else STAGES.index(state.phase)
    targets = (tuple(m for m, v in enumerate(state.observations) if v in (0, 1, 2, 4, 5))
               if kind == RECOVERY_RESOLUTION else tuple(_targets(config, kind)))
    if kind == CONTENT and state.recovery_mode and not _source_available(state, config):
        yield "recover:no_content_source", _begin_recovery(state)
        return
    # A planned normal result after ACCEPT recovery is not historical proof.
    # This knowledge is acquired only by reading an actual normal PUBLICATION
    # or installing one in the current writer round.
    historical_normal = state.normal_publication_seen
    if kind in (COMMIT_PROOF, PUBLICATION, PUBLICATION_PROOF) and not historical_normal and state.recovery_result != 2:
        if not _healthy_content(state, config):
            yield "recover:known_content_damage", _begin_recovery(state)
            return
    if kind == ACCEPT and not state.issued[state.slot]:
        yield "issue_accept", replace(state, issued=_set(state.issued, state.slot, True),
                                      binding=_set(state.binding, state.slot, state.slot + 1))
        return
    for member in targets:
        if not state.online[member]:
            continue
        a = state.artifacts[state.slot][member][kind]
        update = _io_success(a)
        if update is None:
            continue
        step, value = update
        if config.variant == "omit_sync" and step == "directory_sync":
            value = replace(a, done=True)
            step = "skip_directory_sync"
        yield f"{state.phase}:{member}:{step}", _put(state, member, kind, value)
        if state.fault_count < config.fault_budget and step != "skip_directory_sync":
            for side_effect, failed in _io_failures(a, step):
                next_state = _put(state, member, kind, failed)
                yield f"{state.phase}:{member}:{step}:error:{side_effect}", replace(
                    next_state, fault_count=state.fault_count + 1, response="pending")
    complete = all(state.artifacts[state.slot][m][kind].done for m in targets)
    # The mutant confuses quorum decision with all-member completion.
    if config.variant == "shrunken_quorum" and kind == ACCEPT:
        online_count = sum(state.online)
        mask = sum(1 << m for m in targets if _durable(state.artifacts[state.slot][m][kind]))
        mutant_q = online_count // 2 + 1
        if mask.bit_count() >= mutant_q and not state.decision_witness[state.slot]:
            yield "mutant:decide_using_online_quorum", replace(
                state, decision_witness=_set(state.decision_witness, state.slot, mask))
    if config.variant == "early_ack" and kind == ACCEPT and state.decision_witness[state.slot]:
        yield "mutant:ack_at_decision", replace(_apply(state, 1), response="success", phase="DONE")
    if complete:
        if kind == RECOVERY_RESOLUTION:
            mask = sum(1 << m for m in targets if _durable(state.artifacts[state.slot][m][kind]))
            yield "recover:persist_abort_resolution", replace(
                state, results=_set(state.results, state.slot, 3), phase="RESOLVED", response="aborted",
                abort_resolution=_set(state.abort_resolution, state.slot,
                                      state.abort_resolution[state.slot] or mask),
                binding=_set(state.binding, state.slot, state.slot + 1))
        else:
            next_phase = "APPLY" if kind == PUBLICATION_PROOF else STAGES[kind + 1]
            yield f"barrier:{state.phase}", replace(state, phase=next_phase)


def _observation(state: State, member: int):
    if not state.online[member] or state.read_fault[member]:
        return 3
    row = state.artifacts[state.slot][member]
    # Non-record body corruption affects health, not log continuity.
    log = (row[k] for k in (PREPARE, ACCEPT, FINALIZE, PUBLICATION, RECOVERY_RESOLUTION))
    if any(a.entry and a.data != FULL for a in log):
        return 3
    resolution = row[RECOVERY_RESOLUTION]
    if resolution.entry and resolution.data == FULL:
        return 4
    if row[PUBLICATION].entry and row[PUBLICATION].data == FULL:
        return 5 if state.publication_result[state.slot] == 2 else 2
    if row[ACCEPT].entry and row[ACCEPT].data == FULL:
        return 1
    return 0


def _recover_steps(state: State, config: Config):
    if state.phase == "RECOVER_OBSERVE":
        for m in range(config.n):
            if state.observations[m] == -1:
                observed = _observation(state, m)
                yield f"observe:{m}:{observed}", replace(
                    state, observations=_set(state.observations, m, observed),
                    normal_publication_seen=state.normal_publication_seen or observed == 2)
        trusted = sum(v in (0, 1, 2, 4, 5) for v in state.observations)
        if trusted < config.n // 2 + 1:
            return
        if 4 in state.observations:
            yield "recover:preserve_abort_resolution", replace(state, phase="RECOVER_ABORT")
        elif any(v in (1, 2, 5) for v in state.observations):
            result = 1 if 2 in state.observations else (2 if 5 in state.observations else 0)
            yield "recover:preserve_candidate", replace(state, phase="RECOVER_RESOLVE",
                                                        recovery_result=result)
        else:
            mask = sum(1 << m for m, v in enumerate(state.observations) if v == 0)
            yield "recover:abort_evidence", replace(state, phase="RECOVER_ABORT",
                                                    abort_witness=_set(state.abort_witness, state.slot, mask))
    elif state.phase == "RECOVER_RESOLVE":
        # Recovery can decide from any observed ACCEPT, but cannot resume
        # ordinary publication until the fixed member set is present.
        if not all(state.online) or any(state.read_fault):
            return
        # Reprotection reads the remaining actual members before choosing a
        # historical result; global ghost history is never a recovery oracle.
        unread = [m for m, v in enumerate(state.observations) if v in (-1, 3)]
        if unread:
            for m in unread:
                observed = _observation(state, m)
                if observed != state.observations[m]:
                    yield f"observe:{m}:{observed}", replace(
                        state, observations=_set(state.observations, m, observed),
                    normal_publication_seen=state.normal_publication_seen or observed == 2)
            return
        if 4 in state.observations:
            yield "recover:preserve_abort_resolution", replace(state, phase="RECOVER_ABORT")
            return
        rows = state.artifacts[state.slot]
        # Remove only transient error returns, never committed bytes or identity.
        rows = tuple(tuple(replace(a, failed=False, done=_durable(a),
                                   file_synced=a.stable_data == FULL) for a in row) for row in rows)
        artifacts = _set(state.artifacts, state.slot, rows)
        healthy = _source_available(state, config)
        recorded = 1 if 2 in state.observations else (2 if 5 in state.observations else 0)
        if not healthy and not recorded:
            # Explicit degraded recovery remains a committed result. The
            # original candidate is unchanged; metadata still needs all N.
            result = 2
        else:
            result = recorded or 1
        phase = "PREPARE" if result == 2 or recorded else "CONTENT"
        yield "recover:reprotect_fixed_members", replace(state, artifacts=artifacts, phase=phase,
                                                         recovery_result=result)
    elif state.phase == "RECOVER_ABORT":
        for m, row in enumerate(state.artifacts[state.slot]):
            if row[RECOVERY_RESOLUTION].failed:
                yield f"recover:retry_resolution:{m}", _put(
                    state, m, RECOVERY_RESOLUTION, replace(row[RECOVERY_RESOLUTION], failed=False))
        yield from _stage_steps(state, config)


def _power_artifact(a: Artifact, keep: bool, flushed: int):
    # Synced file bytes do not establish a persistent name. An unsynced
    # write can have reached media in full, partially, or not at all.
    entry = a.dir_synced or (a.entry and keep)
    return replace(a, data=flushed, stable_data=flushed, entry=entry,
                   file_synced=False, done=False, failed=False)


def _crashes(state: State, config: Config):
    if state.crash_count >= config.crashes:
        return
    base = _begin_recovery(replace(state, crash_count=state.crash_count + 1))
    process_rows = tuple(tuple(tuple(replace(a, failed=False) for a in row)
                               for row in members) for members in state.artifacts)
    yield "crash:process", replace(base, artifacts=process_rows)
    names = [(s, m, k) for s, members in enumerate(state.artifacts)
             for m, row in enumerate(members) for k, a in enumerate(row)
             if a.entry and not a.dir_synced]
    pending = [(s, m, k) for s, members in enumerate(state.artifacts)
               for m, row in enumerate(members) for k, a in enumerate(row)
               if a.data != a.stable_data and not a.file_synced]
    for name_choices in product((False, True), repeat=len(names)):
        keep = dict(zip(names, name_choices))
        for byte_choices in product((ABSENT, FULL, TRUNCATED, CORRUPT), repeat=len(pending)):
            flushed = dict(zip(pending, byte_choices))
            rows = tuple(tuple(tuple(_power_artifact(a, keep.get((s, m, k), False),
                                                     flushed.get((s, m, k), a.stable_data))
                                     for k, a in enumerate(row)) for m, row in enumerate(members))
                         for s, members in enumerate(state.artifacts))
            suffix = "".join("1" if c else "0" for c in name_choices) or "none"
            bytes_suffix = "".join(str(c) for c in byte_choices)
            action = f"crash:power:{suffix}" + (f":bytes:{bytes_suffix}" if pending else "")
            yield action, replace(base, artifacts=rows)


def _environment(state: State, config: Config):
    if state.fault_count < config.fault_budget:
        for m in range(config.n):
            if state.online[m]:
                yield f"disk:offline:{m}", replace(state, online=_set(state.online, m, False),
                                                  fault_count=state.fault_count + 1)
                if not state.read_fault[m]:
                    yield f"disk:unreadable:{m}", replace(state, read_fault=_set(state.read_fault, m, True),
                                                         fault_count=state.fault_count + 1)
        for s in range(state.slot + 1):
            for m in range(config.replicas):
                a = state.artifacts[s][m][CONTENT]
                if a.entry and a.data == FULL:
                    row = _set(state.artifacts[s][m], CONTENT,
                               replace(a, data=CORRUPT, stable_data=CORRUPT, file_synced=False, done=False))
                    members = _set(state.artifacts[s], m, row)
                    yield f"content:corrupt:{s}:{m}", replace(
                        state, artifacts=_set(state.artifacts, s, members), fault_count=state.fault_count + 1)
        if state.state_versions and not state.index_failed:
            yield "index:fail", replace(state, index_failed=True, fault_count=state.fault_count + 1)
    for m in range(config.n):
        if not state.online[m]:
            yield f"disk:return:{m}", replace(state, online=_set(state.online, m, True))
        if state.read_fault[m]:
            yield f"disk:readable:{m}", replace(state, read_fault=_set(state.read_fault, m, False))


def _client_steps(state: State, config: Config):
    if state.phase == "ACK":
        yield "reply:delivered", replace(state, phase="RESOLVED", response="success")
        yield "reply:lost", replace(state, phase="RESOLVED", response="pending", reply_lost=True)
    if state.phase in ("RESOLVED", "DONE") and config.retries and state.retry_count == 0:
        yield "retry:original_id", replace(state, retry_count=1,
                                            response="success" if state.results[state.slot] in (1, 2) else "aborted")
        yield "retry:digest_mismatch", replace(state, retry_count=1, response="digest_mismatch")
        if config.variant == "lost_binding" and state.results[state.slot] in (1, 2):
            duplicate = _apply(replace(state, binding=_set(state.binding, state.slot, 0)),
                               state.results[state.slot], duplicate=True)
            yield "mutant:retry_without_binding", replace(duplicate, retry_count=1)
    if state.phase == "RESOLVED":
        aborted_prefix_reprotected = all(
            result != 3 or all(_durable(row[RECOVERY_RESOLUTION]) and
                               row[RECOVERY_RESOLUTION].entry and
                               row[RECOVERY_RESOLUTION].data == FULL
                               for row in state.artifacts[slot])
            for slot, result in enumerate(state.results[:state.slot + 1]))
        if (state.slot + 1 < config.operations and all(state.online) and
                not any(state.read_fault) and aborted_prefix_reprotected):
            yield "next_operation", replace(state, slot=state.slot + 1, phase="CONTENT", recovery_mode=False,
                                             recovery_result=0, normal_publication_seen=False, response="none", retry_count=0,
                                             reply_lost=False, observations=(-1,) * config.n)
        else:
            yield "finish", replace(state, phase="DONE")
    if state.phase in STAGES and state.issued[state.slot] and state.response != "pending":
        if config.variant == "timeout_rollback":
            yield "mutant:timeout_rollback", replace(state, results=_set(state.results, state.slot, 3),
                                                     phase="RESOLVED", response="aborted")
        else:
            yield "client:timeout", replace(state, response="pending")
    if state.phase in STAGES and (state.response == "pending" or any(
            a.failed for row in state.artifacts[state.slot] for a in row)):
        # Explicit isolation is a recovery action, not a timeout rollback.
        yield "recover:isolate_writer", _begin_recovery(state)


def successors(state: State, config: Config) -> Iterator[tuple[str, State]]:
    """All enabled labeled actions in deterministic generation order."""
    if state.phase in STAGES:
        # Degraded recovery cannot claim content proofs for missing bytes.
        if state.recovery_result == 2 and state.phase in ("COMMIT_PROOF", "PUBLICATION_PROOF"):
            following = "PUBLICATION" if state.phase == "COMMIT_PROOF" else "APPLY"
            yield f"recover:skip_unavailable_{state.phase.lower()}", replace(state, phase=following)
        else:
            yield from _stage_steps(state, config)
    elif state.phase == "APPLY":
        yield "publish:atomic_state_receipt_event", _apply(state, state.recovery_result or 1)
    elif state.phase.startswith("RECOVER_"):
        yield from _recover_steps(state, config)
    yield from _client_steps(state, config)
    yield from _environment(state, config)
    # Recovery itself is interruptible; even terminal published states can be
    # reopened. No crash is an implicit client acknowledgement.
    if state.phase != "BLOCKED":
        yield from _crashes(state, config)


def state_to_dict(state: State):
    return asdict(state)


def state_from_dict(data):
    value = dict(data)
    value["artifacts"] = tuple(tuple(tuple(Artifact(**a) for a in row) for row in members)
                               for members in value["artifacts"])
    for key in ("online", "read_fault", "issued", "decision_witness", "known_publication", "results",
                "state_versions", "events", "publish_count", "abort_witness", "abort_resolution", "publication_result", "observations", "binding"):
        value[key] = tuple(value[key])
    value["publication_witness"] = tuple(tuple(row) for row in value["publication_witness"])
    value["publication_origin"] = tuple(tuple(row) for row in value["publication_origin"])
    return State(**value)


# Concise aliases for external callers; they do not implement safety properties.
as_dict = state_to_dict
