//! Publication-bound path provenance. Imported evidence never becomes a local
//! native operation; carriers flatten source receipts and typed projections.
use super::HistoryManifest;
use crate::{Error, OperationResult, PathChangeError, Result, State};
use crate::business::history_copy::retained::{self, RetainedPathChange};
use crate::persist::path_changes::{self, invalid};
use data_serialization::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::{HistoryTreeId, OperationId, Principal, Sequence};

pub(crate) const RESTORE_KIND: &str = "history.restore";
pub(crate) const RESTORED_FIELD: &str = "restored_path_audit";
pub(crate) type Sources = BTreeMap<OperationId, PathAuditSource>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct PathAuditSource {
    pub(crate) operation: OperationResult,
    pub(crate) principal: Principal,
    pub(crate) projections: Vec<RetainedPathChange>,
    pub(crate) derivations: BTreeMap<storage_contract::FileId, Vec<OperationId>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PathAuditCarrier {
    Complete {
        published: Sequence,
        sources: Sources,
    },
    /// An admitted native operation can remain usable even when the portable
    /// format cannot safely normalize its mixed imported/native provenance.
    Unsupported {
        operation: OperationResult,
        principal: Principal,
        histories: BTreeSet<HistoryTreeId>,
    },
}

impl PathAuditCarrier {
    pub(crate) fn histories(&self) -> BTreeSet<HistoryTreeId> {
        match self {
            Self::Complete { sources, .. } => sources.values()
                .flat_map(|source| source.projections.iter().map(|row| row.history_tree_id))
                .collect(),
            Self::Unsupported { histories, .. } => histories.clone(),
        }
    }
}

fn projection_key(row: &RetainedPathChange) -> (
    HistoryTreeId, storage_contract::FileId, storage_contract::FilePathEventId,
) {
    (row.history_tree_id, row.file_id, row.source.event_id)
}

/// Complete original receipts are immutable even when only one copied target
/// projection is retained. Portable admission separately requires the full batch.
pub(crate) fn validate_sources(sources: &Sources) -> Result<()> {
    retained::bounded(sources)?;
    let mut remaining = storage_contract::file_lifecycle::MAX_AUDIT_EVENTS;
    let mut remaining_facts = storage_contract::file_lifecycle::MAX_AUDIT_EVENTS;
    let mut remaining_derivation_steps = storage_contract::file_lifecycle::MAX_AUDIT_EVENTS;
    for (id, source) in sources {
        if source.operation.operation_id != *id
            || !path_changes::is_kind(&source.operation.kind)
            || source.projections.is_empty()
        {
            return Err(invalid());
        }
        if source.projections.len() > remaining {
            return Err(Error::PathChange(PathChangeError::Limit));
        }
        remaining -= source.projections.len();
        let facts = path_changes::read(&source.operation)?;
        if facts.len() > remaining_facts {
            return Err(Error::PathChange(PathChangeError::Limit));
        }
        remaining_facts -= facts.len();
        path_changes::batch_transfers(&source.operation, &source.principal)?;
        if facts.iter().any(|fact| fact.author != source.principal)
            || mixed_move(&source.operation)?
        {
            return Err(invalid());
        }
        let projected_files: BTreeSet<_> = source.projections.iter()
            .map(|row| row.file_id).collect();
        if source.derivations.keys().copied().collect::<BTreeSet<_>>() != projected_files {
            return Err(invalid());
        }
        let indexed: BTreeMap<_, _> = facts.iter().map(|fact| (fact.event_id, fact)).collect();
        let mut previous = None;
        for projection in &source.projections {
            let key = projection_key(projection);
            if previous.is_some_and(|previous| previous >= key)
                || indexed.get(&projection.source.event_id).copied() != Some(&projection.source)
            {
                return Err(invalid());
            }
            let chain = &source.derivations[&projection.file_id];
            if chain.len() > remaining_derivation_steps {
                return Err(Error::PathChange(PathChangeError::Limit));
            }
            remaining_derivation_steps -= chain.len();
            validate_derivation(projection, chain)?;
            previous = Some(key);
        }
    }
    Ok(())
}

/// Prove the endpoint identity derivation, not an invented chain of native
/// operation receipts. Restore preserves this chain; a copy appends its own ID.
fn validate_derivation(row: &RetainedPathChange, chain: &[OperationId]) -> Result<()> {
    let mut file = row.source.file_id;
    let mut branch = row.source.branch_id;
    let mut before = row.source.before_version_id;
    let mut after = row.source.after_version_id;
    let identity = crate::business::history_copy::identity;
    for operation in chain {
        file = identity(*operation, &file.to_string())?.parse()?;
        branch = identity(*operation, &branch.to_string())?.parse()?;
        before = identity(*operation, &before.to_string())?.parse()?;
        after = identity(*operation, &after.to_string())?.parse()?;
    }
    if (file, branch, before, after)
        != (row.file_id, row.branch_id, row.before_version_id, row.after_version_id)
        || chain.is_empty() && row.history_tree_id != row.source.history_tree_id
    {
        return Err(invalid());
    }
    Ok(())
}

fn mixed_move(receipt: &OperationResult) -> Result<bool> {
    if receipt.kind != storage_contract::file_relocation::OBJECT_MOVE_KIND {
        return Ok(false);
    }
    // Only decide whether the complete original payload needs an unsupported
    // nested-source proof. This inspection never grants admission or rewrites it.
    let rows = receipt.result.get("retained_lifecycle")
        .and_then(json_codec::Value::as_array).ok_or_else(invalid)?;
    for row in rows {
        let facts = row.get("path_changes")
            .and_then(json_codec::Value::as_array).ok_or_else(invalid)?;
        if !facts.is_empty() {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn merge(target: &mut Sources, source: Sources) -> Result<()> {
    for (id, source) in source {
        match target.entry(id) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(source);
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                let previous = entry.get_mut();
                if previous.operation != source.operation
                    || previous.principal != source.principal
                {
                    return Err(invalid());
                }
                for (file, chain) in source.derivations {
                    if let Some(old) = previous.derivations.insert(file, chain.clone())
                        && old != chain
                    {
                        return Err(invalid());
                    }
                }
                for row in source.projections {
                    if let Some(old) = previous.projections.iter()
                        .find(|old| projection_key(old) == projection_key(&row))
                    {
                        if old != &row {
                            return Err(invalid());
                        }
                    } else {
                        previous.projections.push(row);
                    }
                }
                previous.projections.sort_by_key(projection_key);
            }
        }
    }
    validate_sources(target)
}

pub(crate) fn restored(receipt: &OperationResult) -> Result<Sources> {
    if receipt.kind != RESTORE_KIND {
        return Err(invalid());
    }
    let value = receipt.result.get(RESTORED_FIELD).ok_or_else(invalid)?;
    retained::bounded(value)?;
    let sources: Sources = json_codec::from_value(value.clone())
        .map_err(|source| Error::PathChange(PathChangeError::Encoding(source)))?;
    validate_sources(&sources)?;
    for source in sources.values() {
        if source.operation.sequence >= receipt.sequence
            || source.projections.iter().any(|row| {
                receipt.history_tree_id.as_deref() != Some(row.history_tree_id.to_string().as_str())
            })
        {
            return Err(invalid());
        }
    }
    Ok(sources)
}

pub(crate) fn carrier(state: &State, id: OperationId) -> Result<Option<PathAuditCarrier>> {
    let receipt = state.operations.get(&id).ok_or_else(invalid)?;
    let native = path_changes::is_kind(&receipt.kind);
    let copied = crate::state_projection::lifecycles::history_copy_owner(receipt);
    if !native && receipt.kind != RESTORE_KIND
        && matches!(copied, crate::state_projection::lifecycles::DeletionOwner::Other)
    {
        return Ok(None);
    }
    let principal = state.operation_principals.get(&id).ok_or_else(invalid)?;
    let principal = Principal::new(principal.clone().into())
        .map_err(|source| Error::PathChange(PathChangeError::Principal(source)))?;
    if receipt.operation_id != id {
        return Err(invalid());
    }
    let mut sources = Sources::new();
    if receipt.kind == RESTORE_KIND {
        sources = restored(receipt)?;
    } else if receipt.kind == storage_contract::file_relocation::OBJECT_MOVE_KIND {
        let movement = crate::business::object_move::read_result(receipt)?;
        if movement.retained_lifecycle.iter().any(|prefix| !prefix.path_changes.is_empty()) {
            let histories = movement.path_changes.iter().map(|fact| fact.history_tree_id)
                .chain(movement.retained_lifecycle.iter()
                    .flat_map(|prefix| prefix.path_changes.iter().map(|row| row.history_tree_id)))
                .collect();
            return Ok(Some(PathAuditCarrier::Unsupported {
                operation: receipt.clone(), principal, histories,
            }));
        }
    } else {
        use crate::state_projection::lifecycles::DeletionOwner;
        match copied {
            DeletionOwner::File(file) => sources = retained::read(receipt, file)?.path_audit,
            DeletionOwner::Other => {}
            DeletionOwner::Files(_) | DeletionOwner::Malformed => return Err(invalid()),
        }
    }
    if native {
        let facts = path_changes::read(receipt)?;
        if !facts.is_empty() {
            let mut projections: Vec<_> = facts.into_iter().map(|fact| RetainedPathChange {
                file_id: fact.file_id, history_tree_id: fact.history_tree_id,
                branch_id: fact.branch_id, before_version_id: fact.before_version_id,
                after_version_id: fact.after_version_id, source: fact,
            }).collect();
            projections.sort_by_key(projection_key);
            let derivations = projections.iter().map(|row| (row.file_id, Vec::new())).collect();
            merge(&mut sources, BTreeMap::from([(id, PathAuditSource {
                operation: receipt.clone(), principal, projections, derivations,
            })]))?;
        }
    }
    validate_sources(&sources)?;
    Ok((!sources.is_empty()).then_some(PathAuditCarrier::Complete {
        published: Sequence::new(receipt.sequence), sources,
    }))
}

pub(crate) fn project(
    state: &State,
    history: &str,
) -> Result<BTreeMap<OperationId, PathAuditCarrier>> {
    let mut result = BTreeMap::new();
    for id in state.operations.keys() {
        if let Some(carrier) = carrier(state, *id)?
            && carrier.histories().iter().any(|id| id.to_string() == history)
        {
            result.insert(*id, carrier);
        }
    }
    Ok(result)
}

/// Capture the source evidence through its admitted owner index. This traverses
/// one level of carriers, not mutable source pools or recursive copy receipts.
pub(crate) fn capture(
    source: crate::state_projection::StateProjection<'_>,
    file: &storage_contract::history_window::HistoryFileId,
) -> Result<Sources> {
    let mut result = Sources::new();
    if let Some(rows) = source.owners().file_rows(file.as_str()) {
        let ids: BTreeSet<_> = rows.path_operations.keys()
            .chain(rows.history_copies.keys()).copied().collect();
        for id in ids {
            match carrier(source.state(), id)? {
                Some(PathAuditCarrier::Complete { sources, .. }) => {
                    let mut selected = Sources::new();
                    for (id, mut evidence) in sources {
                        evidence.projections.retain(|row| row.file_id.to_string() == file.as_str());
                        evidence.derivations.retain(|id, _| id.to_string() == file.as_str());
                        if !evidence.projections.is_empty() {
                            selected.insert(id, evidence);
                        }
                    }
                    merge(&mut result, selected)?;
                }
                Some(PathAuditCarrier::Unsupported { .. }) => {
                    return Err(Error::PathChange(PathChangeError::SourceAuditUnsupported));
                }
                None => {}
            }
        }
    }
    validate_sources(&result)?;
    Ok(result)
}

pub(crate) fn capture_mapped(
    source: crate::state_projection::StateProjection<'_>,
    file: &storage_contract::history_window::HistoryFileId,
    history: &str,
    operation: OperationId,
) -> Result<Sources> {
    let mut result = capture(source, file)?;
    let identity = crate::business::history_copy::identity;
    for evidence in result.values_mut() {
        let mut derivations = BTreeMap::new();
        for (file, mut chain) in std::mem::take(&mut evidence.derivations) {
            chain.push(operation);
            derivations.insert(identity(operation, &file.to_string())?.parse()?, chain);
        }
        evidence.derivations = derivations;
        for row in &mut evidence.projections {
            row.file_id = identity(operation, &row.file_id.to_string())?.parse()?;
            row.history_tree_id = history.parse()?;
            row.branch_id = identity(operation, &row.branch_id.to_string())?.parse()?;
            row.before_version_id =
                identity(operation, &row.before_version_id.to_string())?.parse()?;
            row.after_version_id = identity(operation, &row.after_version_id.to_string())?.parse()?;
        }
        evidence.projections.sort_by_key(projection_key);
    }
    validate_sources(&result)?;
    Ok(result)
}

pub(crate) fn facts(sources: &Sources) -> Vec<RetainedPathChange> {
    let mut result: Vec<_> = sources.values()
        .flat_map(|source| source.projections.iter().cloned()).collect();
    result.sort_by_key(|row| (
        row.source.sequence, row.source.operation_id, row.source.event_id,
        row.history_tree_id, row.file_id,
    ));
    result
}

/// Existing copy deletion/undo provenance remains untouched. Path evidence has
/// its own complete source proof, and must describe exactly the old typed rows.
pub(crate) fn validate_copy(prefix: &retained::CopiedLifecycle) -> Result<()> {
    validate_sources(&prefix.path_audit)?;
    if facts(&prefix.path_audit) != prefix.path_changes {
        return Err(invalid());
    }
    Ok(())
}

/// No recursive State/manifest admission: only the exact retained endpoint
/// graph needed by the existing path reference verifier is projected here.
pub(crate) fn validate_manifest(manifest: &HistoryManifest) -> Result<()> {
    if manifest.path_audit.is_empty() {
        return Ok(());
    }
    let mut graph = State::default();
    graph.histories.insert(manifest.history.history_tree_id.clone(), manifest.history.clone());
    graph.branches.extend(manifest.branches.clone());
    graph.versions.extend(manifest.versions.clone());
    graph.commits.extend_shared(&manifest.commits);
    let history: HistoryTreeId = manifest.history.history_tree_id.parse()?;
    for carrier in manifest.path_audit.values() {
        if !carrier.histories().contains(&history) {
            return Err(invalid());
        }
        match carrier {
            PathAuditCarrier::Complete { published, sources } => {
                if published.get() == 0 {
                    return Err(invalid());
                }
                validate_sources(sources)?;
                for source in sources.values() {
                    if source.operation.sequence > published.get() {
                        return Err(invalid());
                    }
                    let selected: Vec<_> = source.projections.iter()
                        .filter(|row| row.history_tree_id == history).cloned().collect();
                    path_changes::retained_references(&graph, &selected, *published)?;
                }
            }
            PathAuditCarrier::Unsupported { operation, principal, histories } => {
                if !mixed_move(operation)? {
                    return Err(invalid());
                }
                let movement = crate::business::object_move::read_result(operation)?;
                if movement.movement.principal != principal.as_str() {
                    return Err(invalid());
                }
                let expected: BTreeSet<_> = movement.path_changes.iter()
                    .map(|row| row.history_tree_id)
                    .chain(movement.retained_lifecycle.iter()
                        .flat_map(|prefix| {
                            prefix.path_changes.iter().map(|row| row.history_tree_id)
                        }))
                    .collect();
                if &expected != histories {
                    return Err(invalid());
                }
            }
        }
    }
    Ok(())
}

/// Export one whole in-scope batch or none of it. Original source envelopes are
/// never filtered; a missing target mapping rejects before any output is made.
pub(crate) fn portable(manifest: &HistoryManifest, sequence: u64) -> Result<Sources> {
    validate_manifest(manifest)?;
    let history: HistoryTreeId = manifest.history.history_tree_id.parse()?;
    let mut result = Sources::new();
    for carrier in manifest.path_audit.values() {
        match carrier {
            PathAuditCarrier::Complete { published, sources } => {
                if published.get() > sequence {
                    return Err(invalid());
                }
                if carrier.histories() != BTreeSet::from([history]) {
                    return Err(Error::PathChange(PathChangeError::PackageAuditUnsupported));
                }
                merge(&mut result, sources.clone())?;
            }
            PathAuditCarrier::Unsupported { .. } => {
                return Err(Error::PathChange(PathChangeError::PackageAuditUnsupported));
            }
        }
    }
    for source in result.values() {
        let facts = path_changes::read(&source.operation)?;
        if facts.iter().any(|fact| !source.projections.iter().any(|row| &row.source == fact)) {
            return Err(Error::PathChange(PathChangeError::PackageAuditUnsupported));
        }
        if let Some(batch) = path_changes::batch_transfers(&source.operation, &source.principal)? {
            // Empty-directory transfers have no file endpoint proof in this
            // format. Do not silently export their unrepresented source data.
            if batch.items.iter().any(|item| item.file_id.is_none_or(|file| {
                !source.projections.iter().any(|row| row.source.file_id == file)
            })) {
                return Err(Error::PathChange(PathChangeError::PackageAuditUnsupported));
            }
        }
    }
    Ok(result)
}

/// One index for one immutable ledger admission. Source receipts are hashed
/// once per encountered carrier, never rediscovered with an all-State scan for
/// every restore. The index is derived evidence, not a persistent authority.
#[derive(Clone, PartialEq, Eq)]
struct SourceProof(String);

pub(crate) struct CollisionIndex<'a> {
    state: &'a State,
    proofs: BTreeMap<OperationId, SourceProof>,
}

impl<'a> CollisionIndex<'a> {
    pub(crate) fn new(state: &'a State) -> Self {
        Self { state, proofs: BTreeMap::new() }
    }

    pub(crate) fn admit(&mut self, sources: &Sources) -> Result<()> {
        for (id, source) in sources {
            if let Some(local) = self.state.operations.get(id)
                && (local != &source.operation
                    || self.state.operation_principals.get(id).map(String::as_str)
                        != Some(source.principal.as_str()))
            {
                return Err(invalid());
            }
            let proof = SourceProof(storage_contract::object_hash(
                "path-audit-source", &(&source.operation, &source.principal),
            )?);
            if let Some(previous) = self.proofs.insert(*id, proof.clone())
                && previous != proof
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
}

pub(crate) fn check_collisions(
    state: &State,
    incoming: &Sources,
    restore_operation: OperationId,
) -> Result<()> {
    if incoming.contains_key(&restore_operation) {
        return Err(invalid());
    }
    let mut index = CollisionIndex::new(state);
    index.admit(incoming)?;
    for id in state.operations.keys() {
        match carrier(state, *id)? {
            Some(PathAuditCarrier::Complete { sources, .. }) => index.admit(&sources)?,
            Some(PathAuditCarrier::Unsupported { operation, .. }) => {
                let movement = crate::business::object_move::read_result(&operation)?;
                for prefix in &movement.retained_lifecycle {
                    index.admit(&prefix.path_audit)?;
                }
            }
            None => {}
        }
    }
    Ok(())
}
