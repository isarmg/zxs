//! Actual read entry for the new container lifecycle. It deliberately does not
//! route Unborn through file_draft_base or create a synthetic historical node.
use crate::business::namespace::working::{WorkingEntry, WorkingNamespace};
use crate::container_storage::{HeldUnbornReader, ObservedContainerReplica};
use crate::engine::MutationPlan;
use crate::engine::file_draft::{Failure, require_initial};
use crate::engine::fresh_layout::containers::published_source;
use crate::persist::file_draft::birth as codec;
use crate::{Engine, Error, Result};
use std::sync::Arc;
use storage_contract::file_draft::{BirthRequest, DraftLocation};
use storage_contract::{CommitId, EntryId, FileId, OperationId, Principal, Sequence};

mod first;
pub(crate) use first::PreparedFirstFile;
pub(super) mod save;

impl Engine {
    /// This is the existing mutation planner, not a second native publisher.
    /// In particular, Existing wins before HEAD/name checks and allocation.
    /// The physical pipeline alone may replace its empty replica map with the
    /// actual EXCL owner occurrence; this plan cannot install names itself.
    pub(crate) fn plan_unborn_birth(
        &self,
        request: &BirthRequest,
        principal: &Principal,
        operation: OperationId,
    ) -> Result<MutationPlan> {
        let plan = self.plan_mutation_digest(
            codec::KIND,
            principal.as_str(),
            operation,
            codec::request(request)?,
            None,
            |engine, next, _| {
                let context = engine.planning_context();
                if context.current_namespaces.is_some() || context.config.generation().is_some() {
                    return Err(Error::FileDraft(Failure::UnsupportedScope));
                }
                let namespace = WorkingNamespace::from_main_state(
                    context.state, request.repository, context.config.max_entries,
                )?;
                let branch = context.state.branches.get(&namespace.branch().to_string())
                    .ok_or_else(crate::business::file_draft::invalid)?;
                if branch.head_commit_id.parse::<CommitId>()? != request.expected_namespace_head {
                    return Err(Error::FileDraft(Failure::SourceChanged));
                }
                namespace.vacant_parent(&request.path)?;
                let source = context.namespace_host_root(request.repository, namespace.branch())?;
                if source.disk().level != 1 || source.disk().upstream_slot_id.is_some() {
                    return Err(Error::FileDraft(Failure::UnsupportedScope));
                }
                source.common()?;
                let birth = crate::business::file_draft::unborn::Birth {
                    repository: request.repository,
                    path: request.path.clone(),
                    expected_namespace_head: request.expected_namespace_head,
                    file: FileId::generate()?,
                    entry: EntryId::generate()?,
                    owner: source.disk().disk_id.parse()?,
                };
                let projected = crate::business::file_draft::unborn::project(
                    context.state,
                    context.config,
                    operation,
                    Sequence::new(context.next_slot),
                    crate::business::file_draft::unborn::Edit::Birth(&birth),
                    context.config.max_entries,
                    context.config.max_file_bytes,
                )?;
                let encoded = codec::encode(request, projected.target(), birth.owner)?;
                source.common()?;
                *next = projected.into_state();
                Ok(encoded)
            },
        )?;
        if let MutationPlan::Prepared { next, prepared, result } = &plan {
            let logical = crate::business::file_draft::unborn::replay_birth(
                &self.config, &self.state, result, principal,
            )?;
            if !prepared.is_empty()
                || !crate::publication_shape::same_transition(&self.state, logical.state(), next)
            {
                return Err(crate::business::file_draft::invalid());
            }
        }
        Ok(plan)
    }

    /// Birth is a real native decision, never a historical version.
    pub fn create_unborn_file(
        &mut self,
        request: &BirthRequest,
        principal: &Principal,
        operation: OperationId,
    ) -> Result<crate::OperationResult> {
        match self.plan_unborn_birth(request, principal, operation)? {
            MutationPlan::Existing(result) => Ok(result),
            MutationPlan::Prepared { next, prepared, result } => {
                self.persist(*next, prepared, result.clone())?;
                Ok(result)
            }
        }
    }

    /// Current descriptive state for an exact namespace occurrence. This does
    /// not open payloads or let a transport mint a save/publication capability.
    pub fn unborn_working_state(
        &self,
        target: &DraftLocation,
    ) -> Result<storage_contract::file_draft::UnbornWorking> {
        let context = self.planning_context();
        require_initial(&context, target)?;
        let namespace = WorkingNamespace::from_main_state(
            context.state, target.repository, context.config.max_entries,
        )?;
        let lookup = name_policy::LookupPath::from(&target.path);
        let Some(WorkingEntry::Unborn { file, record, path }) = namespace.get(&lookup) else {
            return Err(crate::Error::FileDraft(Failure::UnbornRequired));
        };
        if *file != target.file || record.namespace.entry != target.entry || path != &target.path {
            return Err(crate::Error::FileDraft(Failure::WorkingNamespace));
        }
        record.working.unborn().cloned()
            .ok_or(crate::Error::FileDraft(Failure::UnbornRequired))
    }

    pub(crate) fn open_unborn_working(&self, target: &DraftLocation) -> Result<HeldUnbornReader> {
        let context = self.planning_context();
        require_initial(&context, target)?;
        let namespace = WorkingNamespace::from_main_state(
            context.state, target.repository, context.config.max_entries,
        )?;
        let lookup = name_policy::LookupPath::from(&target.path);
        let Some(WorkingEntry::Unborn { file, record, path }) = namespace.get(&lookup) else {
            return Err(crate::Error::FileDraft(Failure::UnbornRequired));
        };
        if *file != target.file || record.namespace.entry != target.entry || path != &target.path {
            return Err(crate::Error::FileDraft(Failure::WorkingNamespace));
        }
        let owner = record.owner_disk.to_string();
        let disk = context.config.disks.iter().find(|disk| disk.disk_id == owner)
            .ok_or(crate::Error::FileDraft(Failure::UnsupportedScope))?;
        let source = published_source(
            context.config, context.disk_resources, context.state, target.file, disk,
        )?;
        let observed = Arc::new(ObservedContainerReplica::observe(source)?);
        observed.open_unborn_working(context.config.max_file_bytes)
    }
}
