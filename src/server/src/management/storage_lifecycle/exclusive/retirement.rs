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

impl ExclusiveExit {
    /// Complete source-checkpoint export, not metadata resumption or foreign
    /// admission. The caller continues to own host exclusion and primary auth.
    pub fn export_terminal_retirement_archive(
        &self,
    ) -> Result<storage_engine::TerminalRetirementArchive, ApiError> {
        let archive = self
            .retirement_owner()?
            .export_retirement_archive()
            .map_err(crate::engine_backend::error)?;
        self.check()?;
        Ok(archive)
    }
}

impl ExclusiveExit {
    /// Builds a full-source proposal, not a native resume selection. No target
    /// directory opens and no request is reserved by this method.
    pub fn prepare_terminal_chain_readmission(
        &self,
        request: storage_contract::storage_readmission::ReadmissionRequest,
    ) -> Result<storage_engine::TerminalChainReadmissionProposal, ApiError> {
        let proposal = self
            .retirement_owner()?
            .prepare_chain_readmission_proposal(
                &request.expected_disk_id,
                &self.identity.principal,
                request.operation_id,
            )
            .map_err(crate::engine_backend::error)?;
        let target = &proposal.preparation().target;
        if target.disk_id != request.expected_disk_id || target.slot_id != request.slot_id {
            return Err(ApiError::conflict(
                ErrorCode::DiskIdentityMismatch,
                "chain readmission proposal selects another original slot/binding",
            ));
        }
        self.check()?;
        Ok(proposal)
    }
}

impl ExclusiveExit {
    pub fn terminal_chain_readmission_status(
        &self,
    ) -> Result<Option<storage_engine::TerminalChainReadmissionStatus>, ApiError> {
        let status = self
            .retirement_owner()?
            .chain_readmission_status()
            .map_err(crate::engine_backend::error)?;
        if status
            .as_ref()
            .is_some_and(|s| s.preparation.principal != self.identity.principal)
        {
            return Err(ApiError::forbidden());
        }
        self.check()?;
        Ok(status)
    }
    /// Explicit preparation of the target's immutable choice, not a Completed
    /// OperationResult. The main service stays unable to enter active storage.
    pub fn synchronize_terminal_chain_readmission(
        &mut self,
        request: storage_contract::storage_readmission::ReadmissionRequest,
        proposal_digest: &str,
    ) -> Result<storage_engine::TerminalChainReadmissionStatus, ApiError> {
        self.check()?;
        let _owner = acquire_owner(&self.management)?;
        require_settled(self.management.path())?;
        let preparation = match self.terminal_chain_readmission_status()? {
            Some(status) => status.preparation,
            None => self
                .prepare_terminal_chain_readmission(request.clone())?
                .preparation()
                .clone(),
        };
        if preparation.operation_id != request.operation_id
            || preparation.proposal_digest != proposal_digest
            || preparation.target.disk_id != request.expected_disk_id
            || preparation.target.slot_id != request.slot_id
            || preparation.principal != self.identity.principal
        {
            return Err(ApiError::conflict(
                ErrorCode::GenerationMismatch,
                "chain readmission target selection differs from the original source proposal",
            ));
        }
        self.auth.revalidate_identity(&self.identity)?;
        // Resolve only the native-selected original pathname after the complete
        // source comparison. Browser-supplied root paths never enter this step.
        let directory =
            PrivateDirectory::open_existing(&preparation.target_root).map_err(storage_error)?;
        let directory = platform_io::fs_safety::linux::OpenAt2Root::from_private_directory(
            directory,
            platform_io::fs_safety::linux::MountPolicy::SameMount,
        )
        .map_err(storage_error)?;
        let Source::Terminal(terminal) = &mut self.source else {
            return Err(terminal_mutation_unavailable());
        };
        let status = terminal
            .synchronize_chain_readmission_target(
                &self.identity.principal,
                request.operation_id,
                proposal_digest,
                storage_engine::PendingMemberDirectory {
                    disk_id: request.expected_disk_id,
                    directory: directory.into_directory(),
                },
            )
            .map_err(crate::engine_backend::error)?;
        self.check()?;
        Ok(status)
    }
}

impl ExclusiveExit {
    /// Native first-slot input preparation only. Requires the already-selected
    /// original actor/target, and never reports an OperationResult or activation.
    pub fn prepare_terminal_chain_first_candidate(
        &mut self,
        request: storage_contract::storage_readmission::ReadmissionRequest,
        proposal_digest: &str,
    ) -> Result<storage_engine::TerminalChainReadmissionStatus, ApiError> {
        self.check()?;
        let _owner = acquire_owner(&self.management)?;
        require_settled(self.management.path())?;
        let preparation = self.terminal_chain_readmission_status()?.ok_or_else(|| {
            ApiError::conflict(ErrorCode::RecoveryRequired,
                "terminal-chain native first inputs require the existing original target decision")
        })?.preparation;
        if preparation.operation_id != request.operation_id
            || preparation.proposal_digest != proposal_digest
            || preparation.target.disk_id != request.expected_disk_id
            || preparation.target.slot_id != request.slot_id
            || preparation.principal != self.identity.principal
        {
            return Err(ApiError::conflict(
                ErrorCode::GenerationMismatch,
                "terminal-chain first inputs differ from the original selected source",
            ));
        }
        self.auth.revalidate_identity(&self.identity)?;
        let directory =
            PrivateDirectory::open_existing(&preparation.target_root).map_err(storage_error)?;
        let directory = platform_io::fs_safety::linux::OpenAt2Root::from_private_directory(
            directory,
            platform_io::fs_safety::linux::MountPolicy::SameMount,
        )
        .map_err(storage_error)?;
        let Source::Terminal(terminal) = &mut self.source else {
            return Err(terminal_mutation_unavailable());
        };
        let status = terminal
            .prepare_chain_readmission_activation(
                &self.identity.principal,
                request.operation_id,
                proposal_digest,
                storage_engine::PendingMemberDirectory {
                    disk_id: request.expected_disk_id,
                    directory: directory.into_directory(),
                },
            )
            .map_err(crate::engine_backend::error)?;
        self.check()?;
        Ok(status)
    }
}

impl ExclusiveExit {
    /// Actual native first-slot Publication under the original stopped target.
    /// This is not Current/writer admission and returns no business completion.
    pub fn publish_terminal_chain_first(
        &mut self,
        request: storage_contract::storage_readmission::ReadmissionRequest,
        proposal_digest: &str,
    ) -> Result<storage_engine::TerminalChainReadmissionStatus, ApiError> {
        self.check()?;
        let _owner = acquire_owner(&self.management)?;
        require_settled(self.management.path())?;
        let preparation = self
            .terminal_chain_readmission_status()?
            .ok_or_else(|| {
                ApiError::conflict(
                    ErrorCode::RecoveryRequired,
                    "terminal-chain Publication requires existing original target and first inputs",
                )
            })?
            .preparation;
        if preparation.operation_id != request.operation_id
            || preparation.proposal_digest != proposal_digest
            || preparation.target.disk_id != request.expected_disk_id
            || preparation.target.slot_id != request.slot_id
            || preparation.principal != self.identity.principal
        {
            return Err(ApiError::conflict(
                ErrorCode::GenerationMismatch,
                "terminal-chain Publication differs from the original selected source",
            ));
        }
        self.auth.revalidate_identity(&self.identity)?;
        let directory =
            PrivateDirectory::open_existing(&preparation.target_root).map_err(storage_error)?;
        let directory = platform_io::fs_safety::linux::OpenAt2Root::from_private_directory(
            directory,
            platform_io::fs_safety::linux::MountPolicy::SameMount,
        )
        .map_err(storage_error)?;
        let Source::Terminal(terminal) = &mut self.source else {
            return Err(terminal_mutation_unavailable());
        };
        let status = terminal
            .publish_chain_readmission_first(
                &self.identity.principal,
                request.operation_id,
                proposal_digest,
                storage_engine::PendingMemberDirectory {
                    disk_id: request.expected_disk_id,
                    directory: directory.into_directory(),
                },
            )
            .map_err(crate::engine_backend::error)?;
        self.check()?;
        Ok(status)
    }
}

impl ExclusiveExit {
    /// Inspect the real published frozen Current while holding the original
    /// stopped directory. Does not consume Stop or install an active Engine.
    pub fn inspect_terminal_chain_current(
        &mut self,
        request: storage_contract::storage_readmission::ReadmissionRequest,
        proposal_digest: &str,
    ) -> Result<storage_engine::TerminalChainPublishedCurrentReport, ApiError> {
        self.check()?;
        let _owner = acquire_owner(&self.management)?;
        require_settled(self.management.path())?;
        let preparation = self
            .terminal_chain_readmission_status()?
            .ok_or_else(|| {
                ApiError::conflict(
                    ErrorCode::RecoveryRequired,
                    "published Current requires original completed Publication",
                )
            })?
            .preparation;
        if preparation.operation_id != request.operation_id
            || preparation.proposal_digest != proposal_digest
            || preparation.target.disk_id != request.expected_disk_id
            || preparation.target.slot_id != request.slot_id
            || preparation.principal != self.identity.principal
        {
            return Err(ApiError::conflict(
                ErrorCode::GenerationMismatch,
                "published Current differs from original selected source",
            ));
        }
        self.auth.revalidate_identity(&self.identity)?;
        let directory =
            PrivateDirectory::open_existing(&preparation.target_root).map_err(storage_error)?;
        let directory = platform_io::fs_safety::linux::OpenAt2Root::from_private_directory(
            directory,
            platform_io::fs_safety::linux::MountPolicy::SameMount,
        )
        .map_err(storage_error)?;
        let terminal = self.retirement_owner()?;
        let reader = terminal
            .open_chain_published_current(
                &self.identity.principal,
                request.operation_id,
                proposal_digest,
                storage_engine::PendingMemberDirectory {
                    disk_id: request.expected_disk_id,
                    directory: directory.into_directory(),
                },
            )
            .map_err(crate::engine_backend::error)?;
        let report = reader.report().map_err(crate::engine_backend::error)?;
        self.check()?;
        Ok(report)
    }
}
