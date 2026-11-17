//! Reconstruct one complete birth using the original request and allocation.
//! The returned logical State has no physical replica feedback or I/O authority.
use crate::business::file_draft::{DraftError as Failure, invalid};
use crate::business::file_draft::unborn::{Birth, Edit, Projection, project};
use crate::persist::file_draft::birth as codec;
use crate::{Error, OperationResult, Result, State, ValidatedConfig};
use storage_contract::{Event, Principal, Sequence};

pub(crate) fn birth(
    config: &ValidatedConfig,
    previous: &State,
    receipt: &OperationResult,
    principal: &Principal,
) -> Result<Projection> {
    require_source(config, previous, receipt)?;
    let decoded = codec::decode(receipt)?;
    if decoded.epoch() != config.membership_epoch()
        || decoded.receipt(receipt.operation_id, Sequence::new(receipt.sequence), principal)?
            != *receipt
    {
        return Err(invalid());
    }
    let request = decoded.request();
    let allocation = Birth {
        repository: request.repository,
        path: request.path.clone(),
        expected_namespace_head: request.expected_namespace_head,
        file: decoded.target().file,
        entry: decoded.target().entry,
        owner: decoded.owner(),
    };
    let projection = project(
        previous, config, receipt.operation_id, Sequence::new(receipt.sequence),
        Edit::Birth(&allocation), config.max_entries, config.max_file_bytes,
    )?;
    if projection.target() != decoded.target() { return Err(invalid()); }
    finish(previous, projection, receipt, principal)
}

pub(crate) fn save(
    config: &ValidatedConfig,
    previous: &State,
    receipt: &OperationResult,
    principal: &Principal,
) -> Result<Projection> {
    require_source(config, previous, receipt)?;
    let decoded = crate::persist::file_draft::unborn_save::decode(receipt)?;
    if decoded.epoch() != config.membership_epoch()
        || decoded.receipt(receipt.operation_id, Sequence::new(receipt.sequence), principal)?
            != *receipt
    {
        return Err(invalid());
    }
    let request = decoded.request();
    let projection = project(
        previous, config, receipt.operation_id, Sequence::new(receipt.sequence),
        Edit::Save {
            target: &request.target, expected: request.expected, manifest: &request.manifest,
        },
        config.max_entries, config.max_file_bytes,
    )?;
    finish(previous, projection, receipt, principal)
}

pub(crate) fn first(
    config: &ValidatedConfig,
    previous: &State,
    receipt: &OperationResult,
    principal: &Principal,
    read: &crate::container_storage::ReadWorkingDigest<'_>,
) -> Result<Projection> {
    require_source(config, previous, receipt)?;
    let decoded = crate::persist::file_draft::unborn_first::decode(previous, receipt)?;
    if decoded.epoch() != config.membership_epoch()
        || decoded.receipt(previous, receipt.operation_id,
            Sequence::new(receipt.sequence), principal)? != *receipt
    {
        return Err(invalid());
    }
    let selection = decoded.selection();
    crate::business::file_draft::selected::require_all(
        previous, std::slice::from_ref(&selection), config.max_entries, config.max_file_bytes,
    )?;
    let projected = project(
        previous, config, receipt.operation_id, Sequence::new(receipt.sequence),
        Edit::FirstCommit(super::FirstCommit {
            target: &selection.target, expected: selection.expected.revision,
            message: decoded.message(), principal, content: decoded.content(), read,
        }),
        config.max_entries, config.max_file_bytes,
    )?;
    finish(previous, projected, receipt, principal)
}

fn require_source(
    config: &ValidatedConfig, previous: &State, receipt: &OperationResult,
) -> Result<()> {
    if config.generation().is_some() {
        return Err(Error::FileDraft(Failure::UnsupportedScope));
    }
    if previous.operations.contains_key(&receipt.operation_id)
        || receipt.sequence <= previous.sequence
    {
        return Err(invalid());
    }
    crate::directory_conversion::require_import_not_stopped(previous, receipt.operation_id)?;
    Ok(())
}

fn finish(
    previous: &State, mut projection: Projection,
    receipt: &OperationResult, principal: &Principal,
) -> Result<Projection> {
    // These are the same ordinary pre-publication guards as the actual planner.
    // The canonical projection does not borrow unrelated tables from `next`.
    crate::file_deletion::freeze::verify(previous, &projection.state)?;
    crate::business::file_tags::preserve_existing(previous, &projection.state)?;
    projection.state.operations.insert(receipt.operation_id, receipt.clone());
    projection.state.operation_principals.insert(receipt.operation_id, principal.as_str().into());
    projection.state.events.push(Event {
        sequence: receipt.sequence,
        operation_id: receipt.operation_id,
        kind: receipt.kind.clone(),
        object_id: receipt.operation_id.to_string(),
    });
    crate::node_retirement_engine::verify(previous, &projection.state)?;
    crate::file_deletion::publication::verify(previous, &projection.state, None)?;
    Ok(projection)
}
