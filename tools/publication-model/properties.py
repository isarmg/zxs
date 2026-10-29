"""独立安全性质：从状态事实和见证检查规范，不调用转换的判断助手。

The transition module owns enabled actions. This module owns the safety claims.
Witnesses are historical observations, not a second implementation of recovery.
No product implementation, fixture or recovery helper is an oracle here.
"""


def check(previous, state, config):
    violations = []

    def require(condition, name, message, spec):
        if not condition:
            violations.append({"name": name, "message": message, "spec": spec})

    quorum = config.n // 2 + 1
    require(len(state.online) == config.n and len(state.observations) == config.n,
            "fixed_membership", "fixed member cardinality changed", "10.2; ADR 0002")
    require(state.crash_count <= config.crashes and state.fault_count <= config.fault_budget,
            "fault_boundary", "transition exceeded declared fault/crash boundary", "model scope")
    require(state.events == state.state_versions, "atomic_publication",
            "event sequence differs from the authoritative published version sequence", "10.3(5); 10.10(5)")
    require(tuple(sorted(set(state.events))) == state.events, "unique_publication",
            "events repeat or violate decision-slot order", "10.2; 10.3(5); 10.10")
    for slot in range(config.operations):
        prefix = f"slot {slot}: "
        result = state.results[slot]
        decision = state.decision_witness[slot]
        if decision:
            require(decision.bit_count() >= quorum, "fixed_quorum",
                    prefix + "decision witness has fewer than fixed Q distinct members", "10.2; 10.6; 10.7")
            require(decision < (1 << config.n), "fixed_membership",
                    prefix + "decision witness contains a nonmember", "10.2")
        if state.issued[slot] or result in (1, 2):
            require(state.binding[slot] == slot + 1, "permanent_binding",
                    prefix + "original operation_id lost its immutable request digest binding", "10.5; 10.6; 18")
        if result == 3 and state.issued[slot]:
            require(state.abort_witness[slot].bit_count() >= quorum, "no_timeout_abort",
                    prefix + "ACCEPT was issued but abort has no complete fixed-Q absence evidence", "10.4; 10.7; 10.11")
            require(not decision, "decided_never_aborts",
                    prefix + "a majority decision was changed to abort", "10.1; 10.4; 10.7")
            require(not state.known_publication[slot], "fixed_result",
                    prefix + "a historical PUBLICATION was changed to abort", "10.10")
        if result == 3:
            require(state.abort_witness[slot].bit_count() >= quorum, "abort_resolution",
                    prefix + "aborted result has no complete fixed-Q absence evidence", "10.7")
            require(state.abort_resolution[slot].bit_count() >= quorum, "abort_resolution",
                    prefix + "aborted result has no fixed-Q durable resolution", "10.7; 10.8")
        count = state.publish_count[slot]
        require(count in (0, 1), "unique_publication", prefix + "operation has multiple business effects", "10.3(5); 18")
        require(state.events.count(slot) == count and state.state_versions.count(slot) == count,
                "atomic_publication", prefix + "State/result/event publication counts differ", "10.3(5); 10.10(5)")
        require((result in (1, 2)) == bool(count), "atomic_publication",
                prefix + "published result and State/event presence differ", "10.10(5)")
        if result in (1, 2):
            require(bool(decision), "fixed_quorum", prefix + "published operation has no majority decision witness", "10.2; 10.3")
            certificate = state.publication_witness[slot]
            require(len(certificate) == config.n, "publication_barrier",
                    prefix + "publication witness does not cover the fixed member set", "10.3; 10.10(3-5)")
            # Indices are part of the documented abstract artifact vocabulary:
            # content, prepare, accept, finalize, commit proof, publication,
            # publication proof. Do not import transition _durable/_targets.
            metadata_mask = (1 << 1) | (1 << 2) | (1 << 3) | (1 << 5)
            content_mask = (1 << 0) | (1 << 4) | (1 << 6)
            for member, observed in enumerate(certificate):
                require(observed & metadata_mask == metadata_mask, "publication_barrier",
                        prefix + f"member {member} lacks durable metadata/publication evidence", "10.3(2-3); 10.10(1,3)")
                if result == 1 and member < config.replicas:
                    require(observed & content_mask == content_mask, "publication_barrier",
                            prefix + f"required member {member} lacks durable content/proof evidence", "10.3(1,2,4); 10.10(2,4)")
            require(state.known_publication[slot] == result, "fixed_result",
                    prefix + "permanent result differs from the historical publication kind", "10.10")
    if previous is not None:
        _check_edge(previous, state, config, require)
    return tuple(violations)


def _check_edge(previous, state, config, require):
    for member, observation in enumerate(state.observations):
        if observation in (0, 1, 2, 4, 5) and previous.observations[member] != observation:
            require(state.online[member] and not state.read_fault[member],
                    "recovery_observation", f"member {member}: unreadable member counted as a trusted observation", "10.6; 10.7")
            row = state.artifacts[state.slot][member]
            complete_log = all(not row[k].entry or row[k].data == 1 for k in (1, 2, 3, 5, 7))
            present = tuple(row[k].entry and row[k].data == 1 for k in (2, 5, 7))
            require(complete_log, "recovery_observation",
                    f"member {member}: installed invalid log counted as a trusted observation", "10.6; 10.7")
            if observation == 0:
                require(not any(present), "recovery_observation",
                        f"member {member}: installed ACCEPT/PUBLICATION/resolution counted as absence", "10.6; 10.7")
            elif observation == 1:
                require(present[0] and not present[1] and not present[2], "recovery_observation",
                        f"member {member}: ACCEPT observation omits or invents stronger records", "10.6; 10.7; 10.10")
            elif observation in (2, 5):
                required_result = 1 if observation == 2 else 2
                require(present[1] and state.publication_result[state.slot] == required_result,
                        "recovery_observation", f"member {member}: PUBLICATION observation differs from record payload", "10.10")
            else:
                require(present[2], "recovery_observation",
                        f"member {member}: abort resolution observation has no complete record", "10.7")
    for slot in range(config.operations):
        prefix = f"slot {slot}: "
        if previous.results[slot] in (1, 2):
            require(state.results[slot] == previous.results[slot], "fixed_result",
                    prefix + "published historical result changed", "10.10; ADR 0005")
            require(state.publication_witness[slot] == previous.publication_witness[slot],
                    "fixed_result", prefix + "historical protection witness changed on retry/recovery/health failure", "10.10; ADR 0005")
        if previous.results[slot] == 3:
            require(state.results[slot] == 3 and state.abort_resolution[slot] == previous.abort_resolution[slot],
                    "abort_resolution", prefix + "durably resolved abort was changed or its evidence replaced", "10.7; 10.8")
        if previous.known_publication[slot]:
            require(state.known_publication[slot] == previous.known_publication[slot], "fixed_result",
                    prefix + "a durable PUBLICATION was reclassified before/after State application", "10.10; ADR 0005")
        if previous.decision_witness[slot]:
            require(state.decision_witness[slot] == previous.decision_witness[slot],
                    "fixed_quorum", prefix + "original decision evidence was discarded or replaced", "10.7; 10.10")
        if previous.issued[slot]:
            require(state.issued[slot], "permanent_binding", prefix + "issued ACCEPT history was lost", "10.4; 10.11")
            require(state.binding[slot] == previous.binding[slot], "permanent_binding",
                    prefix + "request digest binding changed after ACCEPT was issued", "10.5")
        if not previous.publication_origin[slot] and state.publication_origin[slot]:
            observed = _independent_durable_masks(state, slot)
            require(state.publication_origin[slot] == observed, "publication_evidence",
                    prefix + "historical publication origin differs from actual durability facts", "10.3; 10.10")
        elif previous.publication_origin[slot]:
            require(state.publication_origin[slot] == previous.publication_origin[slot], "fixed_result",
                    prefix + "historical publication origin changed", "10.10; ADR 0005")
        if not previous.results[slot] and state.results[slot] in (1, 2):
            actual = _independent_durable_masks(state, slot)
            origin = state.publication_origin[slot]
            carried_bits = (1 << 0) | (1 << 4)
            expected = tuple(bits | ((origin[m] & carried_bits) if origin else 0)
                             for m, bits in enumerate(actual))
            require(state.publication_witness[slot] == expected, "publication_evidence",
                    prefix + "publication certificate differs from physical/current and carried historical evidence", "10.3; 10.10")
    if state.response == "success":
        require(state.results[state.slot] in (1, 2), "success_requires_publication",
                "success response has no permanent published result", "10.1; 10.3; 10.10")
    if state.response == "digest_mismatch" and previous.response != "digest_mismatch":
        require(state.results == previous.results and state.events == previous.events
                and state.state_versions == previous.state_versions and state.binding == previous.binding,
                "permanent_binding", "mismatched digest retry changed authoritative facts", "10.5; 18")
    if state.slot > previous.slot:
        require(previous.results[previous.slot] in (1, 2, 3), "single_decision_slot",
                "next decision slot started before the previous slot converged", "10.2")
        require(all(state.online) and not any(state.read_fault), "fixed_member_writes",
                "next normal operation started without the complete fixed member set", "10.2; 18; ADR 0002")
    if state.index_failed != previous.index_failed and state.index_failed:
        require(state.results == previous.results and state.state_versions == previous.state_versions
                and state.events == previous.events, "index_independence",
                "derived index failure changed an authoritative published fact", "10.10; 18; ADR 0005")
    _check_stage_barrier(previous, state, config, require)


def _independent_durable_masks(state, slot):
    return tuple(sum(1 << kind for kind, artifact in enumerate(member)
                     if artifact.stable_data == 1 and artifact.dir_synced)
                 for member in state.artifacts[slot])


def _check_stage_barrier(previous, state, config, require):
    stages = ("CONTENT", "PREPARE", "ACCEPT", "FINALIZE", "COMMIT_PROOF", "PUBLICATION", "PUBLICATION_PROOF", "APPLY")
    if previous.phase not in stages[:-1]:
        return
    kind = stages.index(previous.phase)
    if state.phase != stages[kind + 1]:
        return
    if previous.recovery_result == 2 and kind in (4, 6):
        return
    targets = range(config.replicas if kind in (0, 4, 6) else config.n)
    for member in targets:
        artifact = state.artifacts[state.slot][member][kind]
        require(artifact.stable_data == 1 and artifact.dir_synced and artifact.entry,
                "durability_barrier", f"{previous.phase}: member {member} advanced without durable complete bytes and directory entry",
                "10.3; 10.5; 10.10")
