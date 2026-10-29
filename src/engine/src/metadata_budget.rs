//! Preflight the exact JSON shapes that publication will persist. Hash values
//! occupy exactly 64 ASCII bytes; every other value comes from the real candidate.
use crate::journal::RecordType;
use crate::{degraded_recovery::RecoveryProof, disk::*, journal::*, *};

fn receipt(
    disk: &DiskConfig,
    kind: RecordType,
    candidate: &Candidate,
    hash: &str,
    object: &str,
) -> Receipt {
    Receipt {
        disk_id: disk.disk_id.clone(),
        slot_id: disk.slot_id.clone(),
        frame_hash: "0".repeat(64),
        record: Record {
            record_type: kind,
            sequence: candidate.sequence,
            candidate_hash: hash.into(),
            object_hash: object.into(),
            previous_frame_hash: "0".repeat(64),
        },
    }
}
/// No future variable-size data is guessed: receipt hashes have fixed hex width,
/// sequence numbers are this candidate's actual decimal strings, and member/slot
/// identifiers, page references, result fields and recovery records are real.
/// The returned bound is exact for the persisted proof JSON shapes.
#[cfg(test)]
pub(crate) fn preflight_publication(
    engine: &Engine,
    candidate: &Candidate,
    result: &OperationResult,
    state: &State,
    recovery: Option<RecoveryProof>,
    maximum: usize,
) -> Result<usize> {
    // Engine fixtures pass published states whose one-shot cold index was
    // consumed; the sizing full encode derives it from the resident tail.
    let state = crate::journal::fixture_indexed_state(state)?;
    preflight_publication_prepared(
        &engine.planning_context(),
        candidate,
        result,
        &state,
        recovery,
        maximum,
        None,
    )
}
/// Recovery sizes tombstones using the candidate's exact stored snapshot graph.
pub(crate) fn preflight_bound_publication(
    engine: &Engine,
    candidate: &Candidate,
    result: &OperationResult,
    state: &State,
    recovery: Option<RecoveryProof>,
    maximum: usize,
) -> Result<usize> {
    preflight_inner(
        &engine.config,
        candidate,
        result,
        state,
        PreflightOptions {
            resources: &engine.disk_resources,
            recovery,
            maximum,
            prepared_graphs: None,
            bound_snapshot: true,
        },
    )
}
pub(crate) fn preflight_publication_prepared(
    context: &crate::engine::PlanningContext<'_>,
    candidate: &Candidate,
    result: &OperationResult,
    state: &State,
    recovery: Option<RecoveryProof>,
    maximum: usize,
    prepared_graphs: Option<(&PreparedSnapshot, &crate::metadata_map::MapRoot)>,
) -> Result<usize> {
    preflight_inner(
        context.config,
        candidate,
        result,
        state,
        PreflightOptions {
            resources: context.disk_resources,
            recovery,
            maximum,
            prepared_graphs,
            bound_snapshot: false,
        },
    )
}
struct PreflightOptions<'a> {
    resources: &'a DiskResources,
    recovery: Option<RecoveryProof>,
    maximum: usize,
    prepared_graphs: Option<(&'a PreparedSnapshot, &'a crate::metadata_map::MapRoot)>,
    bound_snapshot: bool,
}

fn preflight_inner(
    config: &ValidatedConfig,
    candidate: &Candidate,
    result: &OperationResult,
    state: &State,
    options: PreflightOptions<'_>,
) -> Result<usize> {
    let PreflightOptions {
        resources,
        mut recovery,
        maximum,
        prepared_graphs,
        bound_snapshot,
    } = options;
    let mut largest = 0usize;
    let mut check = |bytes: Vec<u8>| {
        largest = largest.max(bytes.len());
    };
    check(encode_metadata(candidate, maximum)?);
    let hash = object_hash("candidate", candidate)?;
    let mut members = config
        .disks
        .iter()
        .filter(|disk| disk.metadata_member)
        .cloned()
        .collect::<Vec<_>>();
    members.sort_by(|a, b| a.disk_id.cmp(&b.disk_id));
    let accepts = if let Some(recovery) = &mut recovery {
        check(encode_metadata(&recovery.plan, maximum)?);
        let plan_hash = object_hash("recovery-plan", &recovery.plan)?;
        recovery.reprotect = members
            .iter()
            .map(|d| receipt(d, RecordType::Reprotect, candidate, &hash, &plan_hash))
            .collect();
        recovery.plan.accepts().to_vec()
    } else {
        members
            .iter()
            .map(|d| receipt(d, RecordType::Accept, candidate, &hash, &hash))
            .collect()
    };
    let finalize = Finalize {
        candidate_hash: hash.clone(),
        accepts,
        recovery_id: recovery.as_ref().map(|r| r.plan.recovery_id.clone()),
    };
    check(encode_metadata(&finalize, maximum)?);
    let finalize_hash = object_hash("finalize", &finalize)?;
    let finalize_receipts = members
        .iter()
        .map(|d| receipt(d, RecordType::Finalize, candidate, &hash, &finalize_hash))
        .collect();
    let mut proofs = Vec::new();
    let mut payload_receipts = Vec::new();
    for evidence in &candidate.versions {
        for slot in &evidence.placement.route_slot_ids {
            if let Some(recovery) = &recovery {
                let healthy = recovery
                    .plan
                    .version_protection
                    .iter()
                    .find(|v| v["version_id"] == evidence.version.version_id)
                    .and_then(|v| v["slots"].as_array())
                    .is_some_and(|slots| {
                        slots
                            .iter()
                            .any(|s| s["slot_id"] == *slot && s["state"] == "COMPLETE_VERIFIED")
                    });
                if !healthy {
                    continue;
                }
            }
            let disk = config
                .disks
                .iter()
                .find(|disk| disk.slot_id.eq_ignore_ascii_case(slot))
                .ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::SlotNotFound,
                        "storage slot is not registered",
                    )
                })?;
            let proof = CommitProof {
                candidate_hash: hash.clone(),
                finalize: finalize.clone(),
                version_hash: object_hash("version", &evidence.version)?,
                disk_id: disk.disk_id.clone(),
                slot_id: slot.clone(),
                route_slot_ids: evidence.placement.route_slot_ids.clone(),
            };
            let proof_bytes = encode_metadata(&proof, maximum)?;
            let proof_len = proof_bytes.len();
            check(proof_bytes);
            payload_receipts.push(PayloadReceipt {
                version_id: evidence.version.version_id.clone(),
                disk_id: disk.disk_id.clone(),
                slot_id: slot.clone(),
                proof_hash: object_hash("commit-proof", &proof)?,
            });
            proofs.push((proof, proof_len));
        }
    }
    let publication = Publication {
        candidate_hash: hash.clone(),
        finalize,
        finalize_receipts,
        payload_receipts,
        result: result.clone(),
        recovery,
    };
    check(encode_metadata(&publication, maximum)?);
    let publication_hash = object_hash("publication", &publication)?;
    let publication_receipts = members
        .iter()
        .map(|d| {
            receipt(
                d,
                RecordType::Publication,
                candidate,
                &hash,
                &publication_hash,
            )
        })
        .collect::<Vec<_>>();
    let seal = PublicationSeal {
        candidate_hash: hash.clone(),
        publication_hash,
        receipts: publication_receipts.clone(),
    };
    check(encode_metadata(&seal, maximum)?);
    // A normal prepared publication consumes exact resident summaries: no
    // State content scan, page decode, or second independent partition pass.
    let recovering = bound_snapshot || publication.recovery.is_some();
    let generated = if !recovering && prepared_graphs.is_none() {
        // The sizing full encode runs against published states, so commit
        // namespaces resolve through verified member replicas.
        let source = crate::namespace::ObjectLoaderSource::new(|kind, hash: &str| {
            crate::publication_evidence::verified_graph_object_at(resources, &members, kind, hash)
        });
        Some((
            prepare_snapshot(state, maximum)?,
            crate::history_package::prepare_history_manifests(state, maximum, &source)?.0,
        ))
    } else {
        None
    };
    let planned = prepared_graphs.or_else(|| {
        generated
            .as_ref()
            .map(|(snapshot, catalog)| (snapshot, catalog))
    });
    let (snapshot, tail, history_proof_bytes) = if recovering {
        let snapshot: Snapshot = crate::publication_evidence::verified_object_at(
            resources,
            &members,
            "snapshot",
            &candidate.snapshot_hash,
        )?;
        validate_snapshot_schema(&snapshot)?;
        let content = crate::metadata_map::MapRoot::load(
            &snapshot.maps["content_states"],
            crate::metadata_map::ProofMeasure::ContentStates,
            |kind, hash| {
                crate::publication_evidence::verified_graph_object_at(
                    resources, &members, kind, hash,
                )
            },
        )?;
        let catalog = crate::metadata_map::MapRoot::load(
            &candidate.history_roots,
            crate::metadata_map::ProofMeasure::Plain,
            |kind, hash| {
                crate::publication_evidence::verified_graph_object_at(
                    resources, &members, kind, hash,
                )
            },
        )?;
        let tail = content.maximum_tombstone_tail();
        (snapshot, tail, catalog.maximum_inclusion_proof_bytes())
    } else {
        let (snapshot, catalog) =
            planned.expect("invariant: normal publication prepared both metadata graphs");
        (
            snapshot.descriptor.clone(),
            snapshot.content_state_budget(),
            catalog.maximum_inclusion_proof_bytes(),
        )
    };
    let package_size = match history_proof_bytes {
        Some(proof_bytes) => crate::history_package::package_proof_bytes(
            candidate,
            &publication,
            &seal,
            proof_bytes,
            maximum,
        )?,
        None => 0, // An empty catalog has no history that can be exported.
    };
    let tombstone_size = if let Some(tail) = tail {
        // The resident maximum pairs a real eligible ID/status with its own
        // exact path, rather than combining unrelated maxima or estimated depth.
        let envelope = json_codec::json!({"version_id":null,"state":null,"candidate":candidate,"publication":&publication,"publication_seal":&seal,"snapshot":&snapshot,"content_state_proof":{"nodes":[]}});
        // Replace null ID (4), null state (4), [] (2) by actual ID/status
        // and the measured nodes contents plus its surrounding [] (2).
        let bytes = canonical_json_value(&envelope)?.len() - 8 + tail;
        if bytes > maximum {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "content tombstone proof exceeds metadata reader budget",
            ));
        }
        bytes
    } else {
        0
    };

    let local_size = preflight_local_publications(
        candidate,
        &publication,
        &publication_receipts,
        &proofs,
        maximum,
    )?;
    Ok(largest
        .max(package_size)
        .max(tombstone_size)
        .max(local_size))
}

/// Each proof was independently encoded and bounded before this envelope stage.
/// Canonical JSON embeds those same bytes, so only the first real envelope needs
/// encoding; its remaining fields are shared unchanged by every local proof.
fn preflight_local_publications(
    candidate: &Candidate,
    publication: &Publication,
    publication_receipts: &[Receipt],
    proofs: &[(CommitProof, usize)],
    maximum: usize,
) -> Result<usize> {
    let Some(((first, first_len), remaining)) = proofs.split_first() else {
        return Ok(0);
    };
    let mut largest = encode_metadata(
        &LocalPublicationRef {
            candidate,
            publication,
            commit_proof: first,
            publication_receipts,
        },
        maximum,
    )?
    .len();
    let common_bytes = largest - *first_len;
    for (_, proof_len) in remaining {
        let bytes = common_bytes
            .checked_add(*proof_len)
            .filter(|bytes| *bytes <= maximum)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::ResourceLimit,
                    format!("serialized metadata exceeds {maximum} byte reader limit"),
                )
            })?;
        largest = largest.max(bytes);
    }
    Ok(largest)
}

#[cfg(test)]
#[path = "../../../test/engine/unit/metadata_budget/tests.rs"]
mod tests;
