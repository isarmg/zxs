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
    let record = previous.file_containers.get(&target.file).ok_or_else(invalid)?;
    super::trace::limit(previous, [(target, record.working.unborn().is_none())])
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
    read: Option<&crate::container_storage::ReadDraftDigest>,
) -> Result<()> {
    let hash = super::draft_content_hash(previous, target, read)?;
    if expected.versions.contains_key(&facts.version.version_id)
        || expected.placements.contains_key(&facts.version.version_id)
        || expected.content_states.contains_key(&facts.version.version_id)
        || facts.version.logical_size != payload.logical_size()
        || facts.version.content_hash != hash
        || facts.allocations.len() > trace_limit(expected, target)?
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
            size: payload.logical_size(), hash,
            chunk_bytes: facts.version.checksum_chunk_bytes,
        },
        &mut effects,
    )?;
    if version != facts.version || placement != facts.placement {
        return Err(invalid());
    }
    // In a combined publication earlier items already occupy their real IDs.
    // Check this accumulating projection, not only the original source table.
    let old_namespace = super::trace::namespace_head(expected, target)?.to_owned();
    let mut replay = Replay::new(expected, &facts.allocations)?;
    publish(
        expected, target, selection.base, version.version_id.parse()?, message, principal,
        operation, &mut |prefix| replay.next(prefix),
    )?;
    replay.finish()?;
    super::trace::require_item(&facts.allocations, true,
        old_namespace != super::trace::namespace_head(expected, target)?, target)
}

/// One selected Born content policy. A descriptor alone cannot build it;
/// the exact admitted context still supplies the actual placement constraints.
pub(super) struct PlannedContent {
    selection: Selection,
    payload: DraftPayload,
}

pub(super) fn plan(
    context: &crate::engine::PlanningContext<'_>,
    request: &storage_contract::file_draft::DraftCommand,
) -> Result<PlannedContent> {
    let storage_contract::file_draft::DraftCommand::Commit { target, base, .. } = request
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
    Ok(PlannedContent { selection, payload: *payload })
}

pub(super) fn build_planned(
    context: &crate::engine::PlanningContext<'_>,
    next: &mut State,
    prepared: &mut Vec<crate::engine::Prepared>,
    request: &storage_contract::file_draft::DraftCommand,
    principal: &str,
    operation: OperationId,
    plan: &PlannedContent,
    source: &crate::engine::ReadSource,
    read: Option<&crate::container_storage::ReadDraftDigest>,
) -> Result<PublishedContent> {
    let storage_contract::file_draft::DraftCommand::Commit { target, base, message, .. } = request
    else {
        return Err(invalid());
    };
    let selection = &plan.selection;
    let payload = plan.payload;
    if selection.base != *base { return Err(Error::FileDraft(Failure::BaseChanged)); }
    let primary = selection.placement.route_slot_ids.first().ok_or_else(invalid)?;
    let hash = super::draft_content_hash(context.state, target, read)?;
    let input = crate::business::VersionBuild {
        history: &selection.history.to_string(), file_id: &target.file.to_string(),
        path: &target.path, source, primary, copies: 1,
        source_ids: vec![base.version_id().to_string()], checkpoint: false,
    };
    // An independent automatic FULL proves the same exact held source and is
    // reread by the ordinary constructor. A hash or raw path cannot enter here.
    let independent = source.independent_automatic(context, target, payload)?;
    let version = match read {
        Some(_) if independent => context.build_version(next, prepared, input)?,
        Some(read) => context.build_draft_version(next, prepared, input, read)?,
        None => context.build_version(next, prepared, input)?,
    };
    if version.logical_size != payload.logical_size() || version.content_hash != hash {
        return Err(Error::FileDraft(Failure::Body));
    }
    let placement = next.placements.get(&version.version_id).cloned().ok_or_else(invalid)?;
    let limit = trace_limit(next, target)?;
    let old_namespace = super::trace::namespace_head(next, target)?.to_owned();
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
    super::trace::require_item(&allocations, true,
        old_namespace != super::trace::namespace_head(next, target)?, target)?;
    Ok(PublishedContent { version, placement, allocations })
}

/// Existing single-file callers keep their original policy/source ordering.
pub(crate) fn build(
    context: &crate::engine::PlanningContext<'_>,
    next: &mut State,
    prepared: &mut Vec<crate::engine::Prepared>,
    request: &storage_contract::file_draft::DraftCommand,
    principal: &str,
    operation: OperationId,
    read: Option<&crate::container_storage::ReadDraftDigest>,
) -> Result<PublishedContent> {
    let plan = plan(context, request)?;
    let source = context.draft_payload_source(request.target().file)?;
    build_planned(context, next, prepared, request, principal, operation, &plan, &source, read)
}
