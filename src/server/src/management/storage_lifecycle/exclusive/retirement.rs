//! Exact native no-voter successor. The management authentication store remains
//! authoritative; no offline helper token fabricates an application identity.
use super::*;
use storage_contract::storage_lifecycle::TerminalRetirementPreview;
use storage_engine::TerminalRetirementPlan;

impl ExclusiveExit {
    fn retirement_owner(&self) -> Result<&TerminalManagement, ApiError> {
        self.check()?;
        match &self.source {
            Source::Terminal(owner) => Ok(owner),
            Source::Active(_) => Err(terminal_mutation_unavailable()),
        }
    }
    pub fn preview_terminal_retirement(
        &self,
        request: ExitRequest,
    ) -> Result<TerminalRetirementPreview, ApiError> {
        active::deregister(request.action)?;
        let owner = self.retirement_owner()?;
        let plan = owner
            .preview_terminal_retirement(
                &request.expected_disk_id,
                &self.identity.principal,
                request.operation_id,
            )
            .map_err(crate::engine_backend::error)?;
        retirement_preview(owner, &plan, request)
    }
    fn exact_retirement_plan(
        &self,
        preview: &TerminalRetirementPreview,
    ) -> Result<TerminalRetirementPlan, ApiError> {
        active::deregister(preview.request.action)?;
        let owner = self.retirement_owner()?;
        let plan = match owner.original_terminal_retirement(
            &self.identity.principal,
            preview.request.operation_id,
            &preview.intent_digest,
        ) {
            Ok(plan) => plan,
            Err(error) if error.code() == storage_engine::ErrorCode::NotFound => owner
                .preview_terminal_retirement(
                    &preview.request.expected_disk_id,
                    &self.identity.principal,
                    preview.request.operation_id,
                )
                .map_err(crate::engine_backend::error)?,
            Err(error) => return Err(crate::engine_backend::error(error)),
        };
        if retirement_preview(owner, &plan, preview.request.clone())? != *preview {
            return Err(ApiError::conflict(
                ErrorCode::GenerationMismatch,
                "terminal retirement preview differs from the exact original native source",
            ));
        }
        Ok(plan)
    }
    pub fn terminal_retirement_result(
        &self,
        preview: &TerminalRetirementPreview,
    ) -> Result<Option<OperationResult>, ApiError> {
        let plan = self.exact_retirement_plan(preview)?;
        self.retirement_owner()?
            .terminal_retirement_result(plan.principal(), plan.operation_id(), plan.digest())
            .map_err(crate::engine_backend::error)
    }
    pub fn complete_terminal_retirement(
        &mut self,
        preview: &TerminalRetirementPreview,
    ) -> Result<OperationResult, ApiError> {
        self.check()?;
        let _owner = acquire_owner(&self.management)?;
        require_settled(self.management.path())?;
        let plan = self.exact_retirement_plan(preview)?;
        self.auth.revalidate_identity(&self.identity)?;
        let Source::Terminal(owner) = &mut self.source else {
            return Err(terminal_mutation_unavailable());
        };
        let result = owner
            .complete_terminal_retirement(&plan)
            .map_err(crate::engine_backend::error)?;
        self.check()?;
        Ok(result)
    }
}
fn retirement_preview(
    owner: &TerminalManagement,
    plan: &TerminalRetirementPlan,
    request: ExitRequest,
) -> Result<TerminalRetirementPreview, ApiError> {
    let target = plan.target();
    if request.expected_disk_id != target.disk_id
        || request.slot_id != target.slot_id
        || request.operation_id != plan.operation_id()
    {
        return Err(ApiError::conflict(
            ErrorCode::DiskIdentityMismatch,
            "terminal retirement target binding differs",
        ));
    }
    let disk = owner
        .original_configuration()
        .disks
        .iter()
        .find(|disk| {
            disk.disk_id == target.disk_id
                && disk.slot_id == target.slot_id
                && disk.binding_generation == target.binding_generation
        })
        .ok_or_else(|| {
            ApiError::conflict(
                ErrorCode::DiskIdentityMismatch,
                "terminal retirement historical definition missing",
            )
        })?;
    let root_path = disk
        .root
        .to_str()
        .ok_or_else(|| {
            ApiError::conflict(
                ErrorCode::DiskIdentityMismatch,
                "terminal registered pathname is not UTF-8",
            )
        })?
        .to_owned();
    Ok(TerminalRetirementPreview {
        request,
        target: ExitTarget {
            pool_id: target.pool_id.clone(),
            disk_id: target.disk_id.clone(),
            slot_id: target.slot_id.clone(),
            binding_generation: target.binding_generation,
            root_path,
        },
        principal: plan.principal().into(),
        intent_digest: plan.digest().into(),
        source_authority_hash: plan.source_authority_hash().into(),
        source_registry_hash: plan.source_registry().root_hash().into(),
        source_registry_generation: plan.source_registry().view_generation(),
        source_sequence: plan.source_sequence(),
        source_topology_generation: plan.source_topology_generation(),
    })
}
