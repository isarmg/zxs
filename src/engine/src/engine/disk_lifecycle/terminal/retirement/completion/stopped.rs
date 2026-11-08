//! Reuse an already certified closed lifetime without inventing another Stop or
//! widening discovery. Its original fixed authority, operation and epoch remain
//! necessary; a Disabled flag or locator hint alone is not closure authority.
use super::*;
use crate::engine::disk_lifecycle::bootstrap::{LoadedLocator, store::BootstrapStore};
pub(in crate::engine::disk_lifecycle::terminal::retirement) const EXISTING: &str =
    "retirement-existing-stop.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct ExistingStop {
    format: Format,
    target: DiskRegistrationIdentity,
    operation_id: OperationId,
    intent_digest: String,
    admission_epoch: Option<String>,
    retirement_digest: String,
    successor_hash: String,
}
pub(super) struct ClosedLifetime {
    operation_id: OperationId,
    intent_digest: String,
    admission_epoch: Option<String>,
}
pub(super) enum StopEvidence {
    Fresh(StopPlan),
    Existing(ExistingStop),
}
impl TerminalManagement {
    pub(super) fn existing_retirement_stop(
        &self,
        locator: &LoadedLocator,
        target: &DiskRegistrationIdentity,
        registry: &TerminalRegistry,
    ) -> Result<Option<ClosedLifetime>> {
        locator
            .require_terminal_retirement_binding(target)
            .map_err(management_error)?;
        let epoch = self
            .verified
            .authority
            .io_admission_epoch(&target.disk_id)?;
        if locator
            .recorded_admission_epoch(target)
            .map_err(management_error)?
            != epoch
        {
            return Err(failure(
                ErrorCode::RecoveryRequired,
                "existing Stop belongs to another current I/O epoch",
            ));
        }
        let Some(hint) = locator.stop_hints().find(|hint| {
            hint.disk_id == target.disk_id
                && hint.slot_id == target.slot_id
                && hint.binding_generation == target.binding_generation
                && hint.admission_epoch.map(|epoch| epoch.to_string()) == epoch
        }) else {
            return Ok(None);
        };
        if registry.registration(target)? != Some(crate::RegistrationMode::Disabled) {
            return Err(failure(
                ErrorCode::RecoveryRequired,
                "only an authenticated Disabled registration may reuse a prior Stop",
            ));
        }
        let digest = hint.intent_digest.to_string();
        let authority = &self.verified.authority;
        if authority.target() == *target
            && authority.operation_id() == hint.operation_id
            && authority.transition_hash()? == digest
            && authority.action() == MembershipExitAction::PlannedOffline
        {
            return Ok(Some(ClosedLifetime {
                operation_id: hint.operation_id,
                intent_digest: digest,
                admission_epoch: epoch,
            }));
        }
        authority
            .predecessor()
            .ok_or_else(|| {
                failure(
                    ErrorCode::RecoveryRequired,
                    "existing Stop has no independently authenticated historical authority",
                )
            })?
            .verify_bootstrap_stop_epoch(target, hint.operation_id, &digest, epoch.as_deref())?;
        Ok(Some(ClosedLifetime {
            operation_id: hint.operation_id,
            intent_digest: digest,
            admission_epoch: epoch,
        }))
    }
    fn expected_existing_stop(
        &self,
        locator: &LoadedLocator,
        plan: &TerminalRetirementPlan,
        successor: &Successor,
    ) -> Result<Option<ExistingStop>> {
        self.existing_retirement_stop(locator, plan.target(), self.retirement_base_registry())?
            .map(|lifetime| {
                Ok(ExistingStop {
                    format: Format::Retirement,
                    target: plan.target().clone(),
                    operation_id: lifetime.operation_id,
                    intent_digest: lifetime.intent_digest,
                    admission_epoch: lifetime.admission_epoch,
                    retirement_digest: plan.digest.clone(),
                    successor_hash: successor.hash()?,
                })
            })
            .transpose()
    }
    pub(super) fn synchronize_retirement_stop(
        &self,
        plan: &TerminalRetirementPlan,
        successor: &Successor,
        source: &VerifiedRetirementStopSource,
    ) -> Result<()> {
        let directory = self.retirement_directory();
        let mut bootstrap = open_store(self.store.system().path())?;
        let fresh = store::optional::<StopPlan>(directory, &directory.path().join(STOP))?;
        let existing =
            store::optional::<ExistingStop>(directory, &directory.path().join(EXISTING))?;
        if fresh.is_some() && existing.is_some() {
            return Err(failure(
                ErrorCode::LogFork,
                "retirement has competing fresh and retained Stop plans",
            ));
        }
        if let Some(fresh) = fresh {
            return self.publish_fresh_retirement_stop(
                &mut bootstrap,
                plan,
                successor,
                source,
                fresh,
            );
        }
        let current = bootstrap
            .reload(&plan.target().pool_id)
            .map_err(management_error)?;
        let expected = self.expected_existing_stop(&current, plan, successor)?;
        if let Some(existing) = &existing
            && expected.as_ref() != Some(existing)
        {
            return Err(failure(
                ErrorCode::LogFork,
                "saved retained Stop differs from independently authenticated current lifetime",
            ));
        }
        if let Some(expected) = expected {
            directory.write_json(&directory.path().join(EXISTING), &expected, false)?;
            self.store.verify_writer(&self._locks)?;
            // Re-establish the original record/head barriers, never append a
            // duplicate Stop operation to an already closed lifetime.
            return bootstrap.sync_current(&current).map_err(management_error);
        }
        let fresh = StopPlan {
            intent_digest: plan.digest.clone(),
            successor_hash: successor.hash()?,
            stop: bootstrap
                .prepare_retirement_stop(&current, source)
                .map_err(management_error)?,
        };
        self.publish_fresh_retirement_stop(&mut bootstrap, plan, successor, source, fresh)
    }
    fn publish_fresh_retirement_stop(
        &self,
        bootstrap: &mut BootstrapStore,
        plan: &TerminalRetirementPlan,
        successor: &Successor,
        source: &VerifiedRetirementStopSource,
        stop: StopPlan,
    ) -> Result<()> {
        require_stop(&stop, plan, successor)?;
        let admitted = bootstrap
            .admit_retirement_stop(&stop.stop, source)
            .map_err(management_error)?;
        let directory = self.retirement_directory();
        directory.write_json(&directory.path().join(STOP), &stop, false)?;
        self.store.verify_writer(&self._locks)?;
        bootstrap.publish_stop(admitted).map_err(management_error)?;
        Ok(())
    }
    pub(super) fn verify_retirement_stop_material(
        &self,
        plan: &TerminalRetirementPlan,
        successor: &Successor,
        source: &VerifiedRetirementStopSource,
    ) -> Result<Option<StopEvidence>> {
        let directory = self.retirement_directory();
        let fresh = store::optional::<StopPlan>(directory, &directory.path().join(STOP))?;
        let existing =
            store::optional::<ExistingStop>(directory, &directory.path().join(EXISTING))?;
        let bootstrap = open_store(self.store.system().path())?;
        match (fresh, existing) {
            (None, None) => Ok(None),
            (Some(stop), None) => {
                require_stop(&stop, plan, successor)?;
                bootstrap
                    .admit_retirement_stop(&stop.stop, source)
                    .map_err(management_error)?;
                Ok(Some(StopEvidence::Fresh(stop)))
            }
            (None, Some(stop)) => {
                let current = bootstrap
                    .reload(&plan.target().pool_id)
                    .map_err(management_error)?;
                if self
                    .expected_existing_stop(&current, plan, successor)?
                    .as_ref()
                    != Some(&stop)
                {
                    return Err(failure(
                        ErrorCode::LogFork,
                        "retained Stop evidence has no exact historical authority",
                    ));
                }
                Ok(Some(StopEvidence::Existing(stop)))
            }
            (Some(_), Some(_)) => Err(failure(
                ErrorCode::LogFork,
                "retirement has two incompatible Stop proofs",
            )),
        }
    }
    pub(super) fn require_durable_retirement_stop(
        &self,
        evidence: &StopEvidence,
        plan: &TerminalRetirementPlan,
        source: &VerifiedRetirementStopSource,
    ) -> Result<()> {
        let bootstrap = open_store(self.store.system().path())?;
        let current = bootstrap
            .reload(&plan.target().pool_id)
            .map_err(management_error)?;
        let (operation, digest) = match evidence {
            StopEvidence::Fresh(stop) => {
                // Retain the exact typed evidence rather than trusting a final
                // marker or independently supplied locator assertion.
                bootstrap
                    .admit_retirement_stop(&stop.stop, source)
                    .map_err(management_error)?;
                (plan.operation_id(), source.digest())
            }
            StopEvidence::Existing(stop) => (
                stop.operation_id,
                IntentDigest::parse(&stop.intent_digest).map_err(management_error)?,
            ),
        };
        if !current.stop_hints().any(|hint| {
            hint.disk_id == plan.target().disk_id
                && hint.slot_id == plan.target().slot_id
                && hint.binding_generation == plan.target().binding_generation
                && hint.operation_id == operation
                && hint.intent_digest == digest
                && hint
                    .admission_epoch
                    .map(|epoch| epoch.to_string())
                    .as_deref()
                    == source.epoch()
        }) {
            return Err(failure(
                ErrorCode::RecoveryRequired,
                "retirement completion has no durable exact closed lifetime",
            ));
        }
        Ok(())
    }
}
