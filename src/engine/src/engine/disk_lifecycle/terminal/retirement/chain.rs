//! Append-only links between completed management retirements. The link itself
//! contains the complete next intent, so no crash can require selecting a new ID
//! or inferring an unpointed directory as authority. No link admits storage I/O.
use super::*;
use std::collections::BTreeSet;

pub(super) const NEXT: &str = "retirement-next.json";
pub(super) const OPERATIONS: &str = "retirement-operations";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Next {
    format: Format,
    pub(super) predecessor_completion: String,
    pub(super) intent: Intent,
    pub(super) intent_digest: String,
}
impl TerminalManagement {
    pub(in crate::engine::disk_lifecycle::terminal) fn retirement_base_registry(
        &self,
    ) -> &TerminalRegistry {
        self.retirement_base_registry
            .as_ref()
            .unwrap_or(&self.verified.registry)
    }
    pub(super) fn retirement_directory(&self) -> &DiskRoot {
        self.retirement_directory
            .as_ref()
            .unwrap_or_else(|| self.store.operation())
    }
    pub(super) fn starts_new_retirement_frame(&self, plan: &TerminalRetirementPlan) -> bool {
        self.retirement_completion
            .as_ref()
            .is_some_and(|complete| complete.plan().operation_id() != plan.operation_id())
    }
    pub(super) fn registry_for_retirement_plan(
        &self,
        plan: &TerminalRetirementPlan,
    ) -> &TerminalRegistry {
        if self.starts_new_retirement_frame(plan) {
            self.registry()
        } else {
            self.retirement_base_registry()
        }
    }
    pub(super) fn namespaces_for_retirement_plan(
        &self,
        plan: &TerminalRetirementPlan,
    ) -> &PreparedCurrentNamespaces {
        if self.starts_new_retirement_frame(plan) {
            self.retirement_completion
                .as_ref()
                .expect("invariant: next retirement has a held completed predecessor")
                .namespaces()
        } else {
            &self.retirement_base_namespaces
        }
    }
    pub(super) fn validate_retirement_proposal(&self, plan: &TerminalRetirementPlan) -> Result<()> {
        if object_hash(DIGEST_KIND, &plan.intent)? != plan.digest {
            return Err(failure(
                ErrorCode::ObjectCorrupt,
                "terminal retirement plan digest differs",
            ));
        }
        if self.starts_new_retirement_frame(plan) {
            self.validate_retirement_at(&plan.intent, self.registry(), &self.state)
        } else {
            self.validate_retirement(&plan.intent)
        }
    }
    pub(super) fn require_no_pending_retirement(&self) -> Result<()> {
        self.require_no_unfollowed_retirement_link()?;
        if self.retirement_completion.is_none() && self.load_terminal_retirement()?.is_some() {
            return Err(failure(
                ErrorCode::RecoveryRequired,
                "recover the exact unfinished terminal retirement before selecting another member",
            ));
        }
        Ok(())
    }
    pub(super) fn require_no_unfollowed_retirement_link(&self) -> Result<()> {
        if self.next_retirement()?.is_some() {
            return Err(failure(
                ErrorCode::RecoveryRequired,
                "terminal successor link exists; recover its exact original request before another observation",
            ));
        }
        Ok(())
    }
    fn next_retirement(&self) -> Result<Option<Next>> {
        let directory = self.retirement_directory();
        store::optional(directory, &directory.path().join(NEXT))
    }
    pub(super) fn select_retirement_frame(&mut self, plan: &TerminalRetirementPlan) -> Result<()> {
        self.validate_retirement_proposal(plan)?;
        if !self.starts_new_retirement_frame(plan) {
            return Ok(());
        }
        self.require_retirement_stop_target(plan.target())?;
        let complete = self.read_retirement_completion()?.ok_or_else(|| {
            failure(
                ErrorCode::RecoveryRequired,
                "next retirement requires its exact completed predecessor",
            )
        })?;
        if self.retirement_completion.as_ref().map(Completed::hash) != Some(complete.hash()) {
            return Err(failure(
                ErrorCode::LogFork,
                "terminal predecessor completion changed before next selection",
            ));
        }
        let next = Next {
            format: Format::Retirement,
            predecessor_completion: complete.hash().into(),
            intent: plan.intent.clone(),
            intent_digest: plan.digest.clone(),
        };
        if self.next_retirement()?.is_some_and(|stored| stored != next) {
            return Err(failure(
                ErrorCode::IdempotencyKeyReused,
                "another terminal successor already owns the next link",
            ));
        }
        self.follow_retirement_link(next)
    }
    fn follow_retirement_link(&mut self, next: Next) -> Result<()> {
        self.store.verify_writer(&self._locks)?;
        let complete = self.retirement_completion.as_ref().ok_or_else(|| {
            failure(
                ErrorCode::RecoveryRequired,
                "a next retirement cannot cross an unfinished predecessor",
            )
        })?;
        if complete.hash() != next.predecessor_completion
            || object_hash(DIGEST_KIND, &next.intent)? != next.intent_digest
        {
            return Err(failure(
                ErrorCode::LogFork,
                "terminal next link differs from its exact predecessor/intent",
            ));
        }
        self.validate_retirement_at(&next.intent, self.registry(), &self.state)?;
        let directory = self.retirement_directory();
        // This full-intent link is the next selection. It is durable before the
        // first new directory. A missing child can recover only this exact ID.
        directory.write_json(&directory.path().join(NEXT), &next, false)?;
        let original = self.store.operation();
        let container_path = original.path().join(OPERATIONS);
        original.private_dir(&container_path)?;
        let container = original.private_child(&container_path)?;
        let child_path = container.path().join(next.intent.operation_id.to_string());
        container.private_dir(&child_path)?;
        let child = container.private_child(&child_path)?;
        self.retirement_base_state = Arc::clone(&self.state);
        let completed = self
            .retirement_completion
            .take()
            .expect("invariant: verified next link has a held predecessor")
            .into_history();
        self.retirement_base_registry = Some(completed.registry);
        self.retirement_base_namespaces = completed.namespaces;
        self.retirement_history.push(completed.historical);
        self.retirement_directory = Some(child);
        // The parent link contains all bytes needed after any child creation
        // interruption; no helper JSON or scan of unknown directories is used.
        self.stage_terminal_retirement(&TerminalRetirementPlan {
            intent: next.intent,
            digest: next.intent_digest,
        })?;
        Ok(())
    }
    pub(in crate::engine::disk_lifecycle::terminal) fn restore_terminal_retirement(
        &mut self,
    ) -> Result<()> {
        loop {
            self.verify_retirement_frame_inventory()?;
            self.restore_retirement_frame()?;
            let Some(next) = self.next_retirement()? else {
                break;
            };
            self.follow_retirement_link(next)?;
        }
        self.verify_retirement_directory_inventory()
    }
    fn verify_retirement_frame_inventory(&self) -> Result<()> {
        if self.retirement_directory.is_none() {
            return Ok(());
        }
        let directory = self.retirement_directory();
        for entry in directory.entries(directory.path())? {
            let entry = entry.map_err(crate::disk::fs_error)?;
            if !entry.as_os_str().to_str().is_some_and(|name| {
                matches!(
                    name,
                    INTENT
                        | SELECTED
                        | completion::SUCCESSOR
                        | completion::STOP
                        | completion::EXISTING
                        | completion::COMPLETE
                        | NEXT
                )
            }) {
                return Err(failure(
                    ErrorCode::RecoveryRequired,
                    "unknown or unfinished terminal successor file",
                ));
            }
        }
        Ok(())
    }
    fn verify_retirement_directory_inventory(&self) -> Result<()> {
        let original = self.store.operation();
        let path = original.path().join(OPERATIONS);
        let expected: BTreeSet<_> = self
            .retirement_history
            .iter()
            .skip(1)
            .map(|complete| complete.plan().operation_id().to_string())
            .chain(
                self.retirement_directory
                    .as_ref()
                    .map(|_| {
                        self.load_terminal_retirement().and_then(|pending| {
                            pending
                                .map(|p| p.plan.operation_id().to_string())
                                .ok_or_else(|| {
                                    failure(
                                        ErrorCode::RecoveryRequired,
                                        "terminal child lost its selected intent",
                                    )
                                })
                        })
                    })
                    .transpose()?,
            )
            .collect();
        if original.optional_metadata(&path)?.is_none() {
            return if expected.is_empty() {
                Ok(())
            } else {
                Err(failure(
                    ErrorCode::RecoveryRequired,
                    "terminal history directory is missing",
                ))
            };
        }
        let container = original.private_child(&path)?;
        let mut present = BTreeSet::new();
        for entry in container.entries(container.path())? {
            let entry = entry.map_err(crate::disk::fs_error)?;
            let name = entry.as_os_str().to_str().ok_or_else(|| {
                failure(
                    ErrorCode::RecoveryRequired,
                    "non-UTF-8 terminal operation directory",
                )
            })?;
            present.insert(name.to_owned());
        }
        if present != expected {
            return Err(failure(
                ErrorCode::RecoveryRequired,
                "terminal operation directories differ from the exact linked history; do not adopt or remove them",
            ));
        }
        Ok(())
    }
    pub(super) fn historical_retirement_plan(
        &self,
        principal: &str,
        operation: OperationId,
        digest: &str,
    ) -> Result<Option<&TerminalRetirementPlan>> {
        let Some(complete) = self
            .retirement_history
            .iter()
            .find(|complete| complete.plan().operation_id() == operation)
        else {
            return Ok(None);
        };
        let plan = complete.plan();
        if plan.principal() != principal || plan.digest() != digest {
            return Err(failure(
                ErrorCode::IdempotencyKeyReused,
                "historical retirement differs from its original actor/operation/digest",
            ));
        }
        Ok(Some(plan))
    }
    pub(super) fn historical_retirement_result(
        &self,
        principal: &str,
        operation: OperationId,
        digest: &str,
    ) -> Result<Option<&OperationResult>> {
        if self
            .historical_retirement_plan(principal, operation, digest)?
            .is_none()
        {
            return Ok(None);
        }
        Ok(self
            .retirement_history
            .iter()
            .find(|complete| complete.plan().operation_id() == operation)
            .map(HistoricalCompleted::result))
    }
}
