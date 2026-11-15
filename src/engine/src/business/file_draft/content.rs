//! Real version construction remains the shared workspace constructor. This
//! adapter seals its body facts, then uses the same branch/namespace publisher.
use super::{DraftError as Failure, invalid};
use super::allocations::{Allocation, Replay};
use super::selection::Selection;
use crate::business::workspace::version_build::{self, Content, Effects, Input};
use crate::{Entry, Error, FileVersion, Placement, Result, State};
use data_serialization::{Deserialize, Serialize};
use std::borrow::Cow;
use storage_contract::file_draft::{DraftBase, DraftLocation, DraftPayload};
use storage_contract::{OperationId, VersionId};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct PublishedContent {
    pub(crate) version: FileVersion,
    pub(crate) placement: Placement,
    pub(crate) allocations: Vec<Allocation>,
}

pub(super) fn publish(
    state: &mut State,
    target: &DraftLocation,
    base: DraftBase,
    version: VersionId,
    message: &str,
    principal: &str,
    operation: OperationId,
    ids: &mut impl FnMut(&str) -> Result<String>,
) -> Result<()> {
    state.file_node_parents.insert(version.to_string(), vec![base.node_id().to_string()]);
    super::super::file_publication::advance_branch_with_ids(
        state, &base.branch_id().to_string(), &version.to_string(), true, principal, operation,
        super::super::file_publication::NodeCommit::Content(message), ids,
    )?;
    super::super::file::publish_selected_file_at_projected_with_ids(
        state, &base.branch_id().to_string(), Some(&target.path), principal, operation, None, ids,
    )?;
    Ok(())
}

pub(crate) fn trace_limit(previous: &State, target: &DraftLocation) -> Result<usize> {
    let repository = previous.repositories.get(&target.repository.to_string())
        .ok_or_else(invalid)?;
    let main = previous.branches.get(&repository.main_branch_id).ok_or_else(invalid)?;
    let entries = previous.commits.get(&main.head_commit_id).ok_or_else(invalid)?.entries.len();
    // One FILE commit/tree, one owner main commit/tree, one root snapshot pair.
    // The root-only pass visits each source file once and may lazily allocate
    // its shared branch/commit/tree. There is no ancestor recursion in this mode.
    entries.checked_mul(3).and_then(|n| n.checked_add(8))
        .ok_or_else(|| Error::FileDraft(Failure::Size))
}

struct ReplayEffects<'a> {
    previous: &'a State,
    facts: &'a PublishedContent,
    source: &'a Placement,
    consumed: bool,
}
impl Effects for ReplayEffects<'_> {
    fn version_id(&mut self) -> Result<VersionId> {
        if self.consumed {
            return Err(invalid());
        }
        self.consumed = true;
        Ok(self.facts.version.version_id.parse()?)
    }
    fn created_at(&mut self) -> Result<String> {
        Ok(self.facts.version.created_at.clone())
    }
    fn manifest_hash(&self) -> Result<String> {
        Ok(self.facts.version.checksum_manifest_hash.clone())
    }
    fn route(&self, primary: &str, copies: u32) -> Result<Vec<String>> {
        if copies != 1 || self.source.route_slot_ids.first().map(String::as_str) != Some(primary) {
            return Err(invalid());
        }
        Ok(self.source.route_slot_ids.clone())
    }
    fn main_entries(&self, branch: &str) -> Result<Cow<'_, [Entry]>> {
        let branch = self.previous.branches.get(branch).ok_or_else(invalid)?;
        let mut entries = self.previous.commits.get(&branch.head_commit_id)
            .ok_or_else(invalid)?.entries.clone();
        let names = crate::business::namespace::container_name::current_names(
            self.previous, &entries,
        )?;
        crate::business::namespace::container_name::apply_file_names(&mut entries, &names)?;
        Ok(Cow::Owned(entries))
    }
    fn binding_generation(&self, slot: &str) -> Result<u64> {
        self.source.binding_generations.get(slot).copied().ok_or_else(invalid)
    }
    fn current(&self) -> Option<&crate::engine::CurrentNamespaces> {
        None
    }
}

pub(super) fn replay(
    previous: &State,
    expected: &mut State,
    target: &DraftLocation,
    selection: &Selection,
    payload: DraftPayload,
    facts: &PublishedContent,
    message: &str,
    principal: &str,
    operation: OperationId,
) -> Result<()> {
    if previous.versions.contains_key(&facts.version.version_id)
        || previous.placements.contains_key(&facts.version.version_id)
        || previous.content_states.contains_key(&facts.version.version_id)
        || facts.version.logical_size != payload.logical_size()
        || facts.version.content_hash != payload.content_hash()
        || facts.allocations.len() > trace_limit(previous, target)?
    {
        return Err(invalid());
    }
    let primary = selection.placement.route_slot_ids.first().ok_or_else(invalid)?;
    let mut effects = ReplayEffects {
        previous, facts, source: &selection.placement, consumed: false,
    };
    let (version, placement) = version_build::construct(
        expected,
        Input {
            history: &selection.history.to_string(), file: &target.file.to_string(),
            path: &target.path, primary, copies: 1,
            sources: vec![selection.base.version_id().to_string()], checkpoint: false,
        },
        Content {
            size: payload.logical_size(), hash: payload.content_hash(),
            chunk_bytes: facts.version.checksum_chunk_bytes,
        },
        &mut effects,
    )?;
    if version != facts.version || placement != facts.placement {
        return Err(invalid());
    }
    let mut replay = Replay::new(previous, &facts.allocations)?;
    publish(
        expected, target, selection.base, version.version_id.parse()?, message, principal,
        operation, &mut |prefix| replay.next(prefix),
    )?;
    replay.finish()
}

pub(crate) fn build(
    context: &crate::engine::PlanningContext<'_>,
    next: &mut State,
    prepared: &mut Vec<crate::engine::Prepared>,
    request: &storage_contract::file_draft::DraftCommand,
    principal: &str,
    operation: OperationId,
) -> Result<PublishedContent> {
    let storage_contract::file_draft::DraftCommand::Commit { target, base, message, .. } = request
    else {
        return Err(invalid());
    };
    let selection = super::selection::resolve(context.state, target)?;
    if selection.base != *base {
        return Err(Error::FileDraft(Failure::BaseChanged));
    }
    let record = context.state.file_containers.get(&target.file).ok_or_else(invalid)?;
    let (_, payload) = record.working.draft().ok_or(Error::FileDraft(Failure::DraftRequired))?;
    let primary = selection.placement.route_slot_ids.first().ok_or_else(invalid)?;
    let (requested_primary, copies) = context.new_file_placement_for_file(
        &target.repository.to_string(), Some(&target.file.to_string()), target.path.as_str(),
    )?;
    let disk = context.disk(&requested_primary)?;
    if copies != 1 || disk.disk_id.parse::<storage_contract::DiskId>()? != record.owner_disk
        || &disk.slot_id != primary || disk.level != 1 || disk.upstream_slot_id.is_some()
        || selection.placement.binding_generations.get(primary) != Some(&disk.binding_generation)
    {
        return Err(Error::FileDraft(Failure::UnsupportedScope));
    }
    let source = context.draft_payload_source(target.file)?;
    let version = context.build_version(
        next, prepared,
        crate::business::VersionBuild {
            history: &selection.history.to_string(), file_id: &target.file.to_string(),
            path: &target.path, source: &source, primary, copies: 1,
            source_ids: vec![base.version_id().to_string()], checkpoint: false,
        },
    )?;
    if version.logical_size != payload.logical_size()
        || version.content_hash != payload.content_hash()
    {
        return Err(Error::FileDraft(Failure::Body));
    }
    let placement = next.placements.get(&version.version_id).cloned().ok_or_else(invalid)?;
    let limit = trace_limit(context.state, target)?;
    let mut allocations = Vec::new();
    publish(
        next, target, *base, version.version_id.parse()?, message, principal, operation,
        &mut |prefix| {
            if allocations.len() == limit {
                return Err(Error::FileDraft(Failure::Size));
            }
            let allocation = Allocation::generate(prefix)?;
            let text = allocation.text(prefix)?;
            allocations.push(allocation);
            Ok(text)
        },
    )?;
    Ok(PublishedContent { version, placement, allocations })
}
