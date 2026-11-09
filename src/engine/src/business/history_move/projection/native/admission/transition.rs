//! Reconstruct the entire native move from its old admitted Current view.
//! A receipt supplies intent only. No draft, stored ID map or serialized view
//! can substitute for the source frontier and the deterministic transformation.
use super::*;

pub(crate) fn verify_current_move_transition(
    previous: &State,
    next: &State,
    view: &crate::engine::CurrentNamespaces,
    receipt: &OperationResult,
    epoch: u64,
) -> Result<()> {
    let body = admit_body(next, receipt)?;
    let node = &body.deletion_node;
    let request = &body.move_request.namespace;
    let principal = next
        .operation_principals
        .get(&receipt.operation_id)
        .ok_or_else(invalid)?;
    if receipt.sequence != next.sequence
        || body.source_sequence != previous.sequence
        || body.membership_epoch != epoch
        || epoch == 0
        || previous.operations.contains_key(&receipt.operation_id)
        || previous
            .operation_principals
            .contains_key(&receipt.operation_id)
    {
        return Err(invalid());
    }
    let sources = history_copy::move_sources(previous, Some(view), request)?;
    if sources.file != body.source_file_id {
        return Err(invalid());
    }
    let repository = previous
        .repositories
        .get(&request.source_repository_id)
        .ok_or_else(invalid)?;
    let target = previous
        .repositories
        .get(&request.target_repository_id)
        .ok_or_else(invalid)?;
    let entries = view.branch_entries(&repository.main_branch_id, &request.expected_source_head)?;
    let repository_id = HistoryRepositoryId::parse(&repository.repository_id)?;
    let file = HistoryFileId::parse(&sources.file)?;
    let path = name_policy::LookupPath::parse(&request.source_path)?;
    require_ordinary_move_source(previous, repository, entries, &path)?;
    let captured = crate::file_deletion::preflight::capture(
        previous,
        repository,
        entries,
        FileDeletionTarget {
            repository_id: &repository_id,
            file_id: &file,
            path: &path,
        },
        usize::MAX,
        &HistoryWindowCancellation::default(),
    )?;
    if captured.occurrences().len() != 1
        || view.backup_configuration_route(&target.primary_slot_id, target.target_copies)?
            != body.move_route.slots
    {
        return Err(invalid());
    }
    for slot in &body.move_route.slots {
        let disk = view.backup_configuration_disk(slot)?;
        if body.move_route.bindings.get(slot) != Some(&disk.identity.binding_generation) {
            return Err(invalid());
        }
    }
    let heads = captured
        .branches()
        .iter()
        .map(|head| (head.branch().branch_id.clone(), head.node_id().to_owned()))
        .collect();
    let writes = compose(
        previous,
        Some(view),
        &ProjectionInput {
            source: ProjectionSource {
                namespace: request,
                file: &file,
                branch_heads: &heads,
                parents: captured.parent_node_ids(),
                versions: &sources.versions,
            },
            route: &body.move_route,
            principal,
            operation: receipt.operation_id,
            sequence: receipt.sequence,
        },
    )?;
    if writes.next.file_deletions.get(file.as_str()) != Some(node)
        || writes.target_file.as_str() != body.file_id
        || writes.version_mapping != body.version_mapping
        || writes.branch_mapping != body.branch_mapping
        || writes.next.branches[&target.main_branch_id].generation != body.branch_generation
        || writes.lifecycle != body.retained_lifecycle
    {
        return Err(invalid());
    }
    let protection = body
        .version_ids
        .iter()
        .map(|id| {
            let placement = writes.next.placements.get(id).ok_or_else(invalid)?;
            let slots = placement
                .route_slot_ids
                .iter()
                .map(|slot| {
                    let disk = view.backup_configuration_disk(slot)?;
                    Ok(Slot {
                        slot_id: slot.clone(),
                        copy_level: disk.level,
                        disk_id: disk.identity.disk_id.clone(),
                        binding_generation: disk.identity.binding_generation,
                        state: SlotState::CompleteVerified,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(Version {
                version_id: id.clone(),
                primary_slot_id: placement.primary_slot_id.clone(),
                target_copies: placement.target_copies,
                required_slots: placement.route_slot_ids.clone(),
                topology_generation: placement.topology_generation,
                slots,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    if protection != body.version_protection {
        return Err(invalid());
    }
    let mut expected = writes.next;
    expected
        .operations
        .insert(receipt.operation_id, receipt.clone());
    expected
        .operation_principals
        .insert(receipt.operation_id, principal.clone());
    expected.events.push(Event {
        sequence: receipt.sequence,
        operation_id: receipt.operation_id,
        kind: KIND.into(),
        object_id: body.commit_id,
    });
    history_copy::retained::bounded(&expected)?;
    history_copy::retained::bounded(next)?;
    if &expected != next {
        return Err(invalid());
    }
    Ok(())
}
