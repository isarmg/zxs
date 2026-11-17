//! First FILE history for one retained worktree birth, never ordinary main.
use crate::business::file_draft::{PublishedContent, invalid};
use crate::container_storage::{PreparedUnbornFull, ReadWorkingDigest};
use crate::engine::{PlanningContext, Prepared, ReadSource};
use crate::persist::snapshot_worktree::added_commit::{AddedCommit, AddedFirst};
use crate::persist::snapshot_worktree::{self as codec, Record};
use crate::{Entry, NamespaceEntryKind, OperationResult, Result, State, ValidatedConfig};
use std::sync::Arc;
use storage_contract::file_container::FileContainerRecord;
use storage_contract::{FileId, Principal, Sequence};

pub(crate) fn original(
    state: &State,
    first: &AddedFirst,
    principal: &Principal,
) -> Result<AddedCommit> {
    let receipt = state.operations.get(&first.reserve).ok_or_else(invalid)?;
    let decoded = super::decode_exact(receipt, principal)?;
    if decoded.record.commitment().map_err(super::encoding)? != first.reservation {
        return Err(invalid());
    }
    let Record::ReserveAddedCommit(plan) = decoded.record else {
        return Err(invalid());
    };
    Ok(plan)
}

fn publish_graph(
    previous: &State,
    next: &mut State,
    plan: &AddedCommit,
    principal: &Principal,
    content: &PublishedContent,
) -> Result<()> {
    let record = super::selected(previous, &plan.added.target)?;
    if record.created_by != plan.added.birth
        || record.history_tree_id != plan.history
        || record.working.unborn() != Some(&plan.added.working)
        || !content.allocations.is_empty()
    {
        return Err(invalid());
    }
    let history = previous
        .histories
        .get(&plan.history.to_string())
        .ok_or_else(invalid)?;
    let parent = previous
        .commits
        .get(&history.root_commit_id)
        .ok_or_else(invalid)?;
    let entry = Entry {
        entry_id: plan.added.target.entry.to_string(),
        path: plan.added.target.path.clone(),
        kind: NamespaceEntryKind::File,
        file_id: plan.added.target.file.to_string(),
        version_id: content.version.version_id.clone(),
        size: content.version.logical_size,
    };
    crate::business::file::initialize_file_root(
        next,
        parent,
        &entry,
        vec![entry.clone()],
        crate::business::file::FileRootIds {
            branch: plan.file_branch,
            commit: plan.file_commit,
            tree: plan.file_tree,
        },
        plan.file_operation,
        principal,
    )?;
    // These mirrors retain the exact birth-local spelling for historical
    // naming. They do not create an ordinary entry or choose a physical parent:
    // the original Worktree namespace still selects the held deleted-data inode.
    next.file_locations.insert(
        entry.file_id.clone(),
        plan.added.target.path.as_str().to_owned(),
    );
    next.file_names.insert(
        entry.file_id.clone(),
        record.namespace.name.as_str().to_owned(),
    );
    next.file_selections
        .insert(entry.file_id.clone(), plan.file_branch.to_string());
    next.file_containers
        .get_mut(&plan.added.target.file)
        .ok_or_else(invalid)?
        .working =
        plan.added.working.after_first_commit().map_err(|source| {
            crate::Error::FileDraft(crate::DraftError::RevisionExhausted(source))
        })?;
    Ok(())
}

pub(crate) fn build(
    context: &PlanningContext<'_>,
    next: &mut State,
    prepared: &mut Vec<Prepared>,
    plan: &AddedCommit,
    principal: &Principal,
    material: &Arc<PreparedUnbornFull>,
) -> Result<PublishedContent> {
    material.require_context(context)?;
    let record = super::selected(context.state, &plan.added.target)?;
    material
        .reader()
        .require_record(plan.added.target.file, record)?;
    let disk = context
        .config
        .disks
        .iter()
        .find(|disk| disk.disk_id == record.owner_disk.to_string())
        .ok_or_else(invalid)?;
    if record.working.unborn() != Some(&plan.added.working)
        || next.sequence != context.next_slot
        || disk.level != 1
        || disk.upstream_slot_id.is_some()
    {
        return Err(invalid());
    }
    let source = ReadSource::working_full(Arc::clone(material));
    let version = context.build_version(
        next,
        prepared,
        crate::business::VersionBuild {
            history: &plan.history.to_string(),
            file_id: &plan.added.target.file.to_string(),
            path: &plan.added.target.path,
            source: &source,
            primary: &disk.slot_id,
            copies: 1,
            source_ids: Vec::new(),
            checkpoint: false,
        },
    )?;
    let read = material.digest()?;
    if version.logical_size != read.source().logical_size()
        || version.content_hash != read.content_hash()
    {
        return Err(invalid());
    }
    let placement = next
        .placements
        .get(&version.version_id)
        .cloned()
        .ok_or_else(invalid)?;
    let content = PublishedContent {
        version,
        placement,
        allocations: Vec::new(),
    };
    publish_graph(context.state, next, plan, principal, &content)?;
    material.require_context(context)?;
    Ok(content)
}

/// Whole-State cold/hot reconstruction consumes the real predecessor digest.
pub(crate) fn replay(
    config: &ValidatedConfig,
    previous: &State,
    receipt: &OperationResult,
    principal: &Principal,
    read: &ReadWorkingDigest<'_>,
) -> Result<(State, FileId)> {
    let decoded = super::decode_exact(receipt, principal)?;
    let Record::PublishAddedFile(first) = &decoded.record else {
        return Err(invalid());
    };
    let plan = original(previous, first, principal)?;
    let file = plan.added.target.file;
    if config.generation().is_some()
        || receipt.operation_id != plan.file_operation
        || decoded.membership_epoch != config.membership_epoch()
        || previous.operations.contains_key(&receipt.operation_id)
        || receipt.sequence <= previous.sequence
        || decoded.protection
            != [crate::persist::file_draft::content_protection_for_file(
                previous,
                file,
                &first.content,
            )?]
    {
        return Err(invalid());
    }
    let record = super::selected(previous, &plan.added.target)?;
    let mut next =
        crate::business::file_draft::unborn::begin(previous, Sequence::new(receipt.sequence))?;
    crate::business::file_draft::unborn::replay_first_version(
        previous,
        config,
        &mut next,
        file,
        &plan.added.target.path,
        record,
        &first.content,
        read,
    )?;
    publish_graph(previous, &mut next, &plan, principal, &first.content)?;
    crate::container_storage::capture_changed_names(previous, &mut next);
    let next =
        crate::business::file_draft::unborn::finish_state(previous, next, receipt, principal)?;
    Ok((next, file))
}

/// Retained ownership remains anchored to birth even when another instance
/// later references this FileId. This is not permission to select by that ID.
pub(crate) fn validate_retained(
    state: &State,
    file: FileId,
    record: &FileContainerRecord,
    birth: &crate::persist::snapshot_worktree::files::WorktreeFileBirth,
) -> Result<()> {
    let key = file.to_string();
    if state.file_locations.get(&key).map(String::as_str) != Some(birth.target.path.as_str())
        || state.file_names.get(&key).map(String::as_str) != Some(record.namespace.name.as_str())
    {
        return Err(invalid());
    }
    let mut found = None;
    for receipt in state
        .operations
        .values()
        .filter(|receipt| receipt.kind == codec::ADDED_FIRST)
    {
        let principal = state
            .operation_principals
            .get(&receipt.operation_id)
            .ok_or_else(invalid)?
            .parse::<Principal>()
            .map_err(|_| invalid())?;
        let decoded = super::decode_exact(receipt, &principal)?;
        let Record::PublishAddedFile(first) = decoded.record else {
            return Err(invalid());
        };
        if first.content.version.file_id != file.to_string() {
            continue;
        }
        let plan = original(state, &first, &principal)?;
        let version = state
            .versions
            .get(&first.content.version.version_id)
            .ok_or_else(invalid)?;
        let branch = state
            .branches
            .get(&plan.file_branch.to_string())
            .ok_or_else(invalid)?;
        let commit = state
            .commits
            .get(&plan.file_commit.to_string())
            .ok_or_else(invalid)?;
        if found.replace(receipt.operation_id).is_some()
            || plan.added.target != birth.target
            || plan.added.birth != record.created_by
            || plan.file_operation != receipt.operation_id
            || plan.history != record.history_tree_id
            || version != &first.content.version
            || version.version_sequence != receipt.sequence
            || !version.source_version_ids.is_empty()
            || branch.file_id.as_deref() != Some(file.to_string().as_str())
            || branch.history_tree_id != record.history_tree_id.to_string()
            || commit.operation_id != receipt.operation_id
            || commit.sequence != receipt.sequence
            || commit.entries.len() != 1
            || commit.entries[0].file_id != file.to_string()
            || commit.entries[0].entry_id != birth.target.entry.to_string()
            || commit.entries[0].version_id != version.version_id
            || record.working.revision().generation().get()
                <= plan.added.working.revision().generation().get()
        {
            return Err(invalid());
        }
    }
    if found.is_none() {
        return Err(invalid());
    }
    Ok(())
}
