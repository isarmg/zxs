//! Canonical native birth/save projection for an exact worktree-owned occurrence.
//! These are descriptive rows; only the held root and native FILE preparation
//! may attach the actual deleted-data inode and publish its three-scope material.
pub(crate) mod first;

use crate::business::file_draft::{DraftError as Failure, invalid};
use crate::persist::snapshot_worktree::files::{WorktreeFileBirth, WorktreeFileSave};
use crate::persist::snapshot_worktree::{self as codec, Record};
use crate::{Error, OperationResult, Result, State, ValidatedConfig};
use storage_contract::file_container::{ContainerNamespace, ContainerParent, FileContainerRecord};
use storage_contract::file_container_tags::ContainerTags;
use storage_contract::file_draft::{FileWorkingState, UnbornBody, UnbornWorking};
use storage_contract::snapshot_worktree::additions::WorktreeFileTarget;
use storage_contract::{FileId, Principal, Sequence};

pub(crate) fn failure(source: crate::snapshot_worktree::RegistrationError) -> Error {
    Error::FileDraft(Failure::Worktree(Box::new(source.into())))
}
fn encoding(source: codec::CodecError) -> Error {
    failure(crate::snapshot_worktree::RegistrationError::Codec(source))
}

pub(crate) fn birth(
    previous: &State,
    config: &ValidatedConfig,
    sequence: Sequence,
    plan: &WorktreeFileBirth,
) -> Result<State> {
    if config.generation().is_some() {
        return Err(Error::FileDraft(Failure::UnsupportedScope));
    }
    let operation = plan.request.operation_id;
    if plan.target.worktree != plan.request.target
        || plan.target.path != plan.request.path
        || !crate::container_storage::namespace_host::ordinary_path(&plan.target.path)
        || plan.owner != plan.allocation.registration.disk
    {
        return Err(invalid());
    }
    super::unborn::require_fresh(previous, plan.target.file, plan.target.entry)?;
    let repository = previous
        .repositories
        .get(&plan.repository.to_string())
        .ok_or_else(invalid)?;
    let disk = config
        .disks
        .iter()
        .find(|disk| disk.slot_id == repository.primary_slot_id)
        .ok_or_else(invalid)?;
    if !repository.file_manager_root
        || repository.main_branch_id != plan.branch.to_string()
        || repository.history_tree_id != plan.history.to_string()
        || disk.disk_id != plan.owner.to_string()
        || disk.level != 1
        || disk.upstream_slot_id.is_some()
        || config.pool_id != plan.allocation.registration.pool.to_string()
    {
        return Err(Error::FileDraft(Failure::UnsupportedScope));
    }
    let name = name_policy::LookupName::parse(plan.request.path.file_name())?;
    if plan.allocation.name.original_name() != name.as_str() {
        return Err(invalid());
    }
    let mut next = super::unborn::begin(previous, sequence)?;
    next.file_containers.insert(
        plan.target.file,
        FileContainerRecord {
            history_tree_id: plan.history,
            owner_disk: plan.owner,
            created_by: operation,
            namespace: ContainerNamespace {
                repository: plan.repository,
                branch: plan.branch,
                entry: plan.target.entry,
                parent: ContainerParent::Worktree {
                    target: plan.target.worktree,
                    path: plan.target.path.clone(),
                    name: plan.allocation.name.clone(),
                },
                name,
            },
            working: FileWorkingState::Unborn {
                working: UnbornWorking::birth(),
            },
            replicas: std::collections::BTreeMap::new(),
        },
    );
    next.container_tags
        .insert(plan.target.file, ContainerTags::empty());
    crate::container_storage::capture_changed_names(previous, &mut next);
    Ok(next)
}

pub(crate) fn selected<'a>(
    state: &'a State,
    target: &WorktreeFileTarget,
) -> Result<&'a FileContainerRecord> {
    let record = state
        .file_containers
        .get(&target.file)
        .ok_or_else(invalid)?;
    let ContainerParent::Worktree {
        target: instance,
        path,
        ..
    } = &record.namespace.parent
    else {
        return Err(Error::FileDraft(Failure::UnbornRequired));
    };
    if target.worktree != *instance
        || target.path != *path
        || target.entry != record.namespace.entry
    {
        return Err(Error::FileDraft(Failure::SourceChanged));
    }
    validate_record(state, target.file, record)?;
    Ok(record)
}

pub(crate) fn save(
    previous: &State,
    config: &ValidatedConfig,
    sequence: Sequence,
    plan: &WorktreeFileSave,
) -> Result<State> {
    if config.generation().is_some() {
        return Err(Error::FileDraft(Failure::UnsupportedScope));
    }
    let request = &plan.request;
    let record = selected(previous, &request.target)?;
    if record.created_by != plan.birth || record.working.revision() != request.working {
        return Err(Error::FileDraft(Failure::SourceChanged));
    }
    let working = record
        .working
        .unborn()
        .ok_or_else(invalid)?
        .saved(
            request.target.file,
            request.manifest.clone(),
            config.max_file_bytes,
        )
        .map_err(|source| Error::FileDraft(Failure::WorkingManifest(source)))?;
    let mut next = super::unborn::begin(previous, sequence)?;
    next.file_containers
        .get_mut(&request.target.file)
        .ok_or_else(invalid)?
        .working = FileWorkingState::Unborn { working };
    crate::container_storage::capture_changed_names(previous, &mut next);
    Ok(next)
}

pub(crate) fn replay(
    config: &ValidatedConfig,
    previous: &State,
    receipt: &OperationResult,
    principal: &Principal,
) -> Result<(State, FileId)> {
    if previous.operations.contains_key(&receipt.operation_id)
        || receipt.sequence <= previous.sequence
    {
        return Err(invalid());
    }
    crate::directory_conversion::require_import_not_stopped(previous, receipt.operation_id)?;
    let decoded = decode_exact(receipt, principal)?;
    if decoded.membership_epoch != config.membership_epoch() {
        return Err(invalid());
    }
    let sequence = Sequence::new(receipt.sequence);
    let (next, file) = match &decoded.record {
        Record::BirthFile(plan) if plan.request.operation_id == receipt.operation_id => {
            (birth(previous, config, sequence, plan)?, plan.target.file)
        }
        Record::SaveFile(plan) if plan.request.manifest.operation() == receipt.operation_id => (
            save(previous, config, sequence, plan)?,
            plan.request.target.file,
        ),
        _ => return Err(invalid()),
    };
    Ok((
        super::unborn::finish_state(previous, next, receipt, principal)?,
        file,
    ))
}

fn decode_exact(receipt: &OperationResult, principal: &Principal) -> Result<codec::Decoded> {
    let decoded = codec::decode(receipt)
        .map_err(encoding)?
        .ok_or_else(invalid)?;
    if decoded
        .receipt(
            receipt.operation_id,
            Sequence::new(receipt.sequence),
            principal,
        )
        .map_err(encoding)?
        != *receipt
    {
        return Err(invalid());
    }
    Ok(decoded)
}

pub(crate) fn validate_record(
    state: &State,
    file: FileId,
    record: &FileContainerRecord,
) -> Result<()> {
    let receipt = state
        .operations
        .get(&record.created_by)
        .ok_or_else(invalid)?;
    let principal = state
        .operation_principals
        .get(&record.created_by)
        .ok_or_else(invalid)?
        .parse::<Principal>()
        .map_err(|_| invalid())?;
    let decoded = decode_exact(receipt, &principal)?;
    let Record::BirthFile(plan) = decoded.record else {
        return Err(invalid());
    };
    let expected_parent = ContainerParent::Worktree {
        target: plan.target.worktree,
        path: plan.target.path.clone(),
        name: plan.allocation.name.clone(),
    };
    if receipt.sequence > state.sequence
        || plan.target.file != file
        || plan.target.entry != record.namespace.entry
        || plan.history != record.history_tree_id
        || plan.owner != record.owner_disk
        || plan.repository != record.namespace.repository
        || plan.branch != record.namespace.branch
        || record.namespace.parent != expected_parent
        || record.namespace.name.as_str() != plan.request.path.file_name()
        || plan.target.worktree != plan.request.target
        || plan.target.path != plan.request.path
    {
        return Err(invalid());
    }
    if record.working.unborn().is_none() {
        return first::validate_retained(state, file, record, &plan);
    }
    let key = file.to_string();
    if state
        .versions
        .values()
        .any(|version| version.file_id == key)
        || state
            .branches
            .values()
            .any(|branch| branch.file_id.as_deref() == Some(key.as_str()))
        || state
            .file_metadata_nodes
            .values()
            .any(|node| node.file_id == key)
        || state
            .file_branch_merges
            .values()
            .any(|node| node.file_id == key)
        || state.file_deletions.contains_key(&key)
        || state.file_selections.contains_key(&key)
        || state.file_locations.contains_key(&key)
        || state.file_names.contains_key(&key)
        || state.file_notes.contains_key(&key)
        || state.file_backup_paths.contains_key(&key)
        || state.file_backup_policies.contains_key(&key)
        || state.file_write_origins.contains_key(&key)
        || state.container_tags.get(&file) != Some(&ContainerTags::empty())
    {
        return Err(invalid());
    }
    let working = record.working.unborn().ok_or_else(invalid)?;
    match working.body() {
        UnbornBody::Empty {} if working == &UnbornWorking::birth() => Ok(()),
        UnbornBody::Empty {} => Err(invalid()),
        UnbornBody::Saved { manifest } => {
            let saved = state
                .operations
                .get(&manifest.operation())
                .ok_or_else(invalid)?;
            let principal = state
                .operation_principals
                .get(&manifest.operation())
                .ok_or_else(invalid)?
                .parse::<Principal>()
                .map_err(|_| invalid())?;
            let decoded = decode_exact(saved, &principal)?;
            let Record::SaveFile(save) = decoded.record else {
                return Err(invalid());
            };
            if saved.sequence <= receipt.sequence
                || saved.sequence > state.sequence
                || save.birth != record.created_by
                || save.request.target != plan.target
                || save.request.manifest != *manifest
                || save
                    .request
                    .working
                    .next()
                    .map_err(|source| Error::FileDraft(Failure::RevisionExhausted(source)))?
                    != working.revision()
            {
                return Err(invalid());
            }
            Ok(())
        }
    }
}
