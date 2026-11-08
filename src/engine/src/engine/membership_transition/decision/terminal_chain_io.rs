//! A separate immutable choice for the exact post-Publication epoch plan.
//! An earlier chain membership vote cannot be relabelled as I/O admission.
use super::*;

fn record(
    epoch: &str,
    identity: &MemberIdentity,
    plan: &str,
    acceptance: &str,
) -> Result<DecisionRecord> {
    validate_hash(epoch)?;
    validate_hash(plan)?;
    validate_hash(acceptance)?;
    Ok(DecisionRecord {
        format: DecisionFormat::Decision,
        transition_hash: epoch.into(),
        identity: identity.clone(),
        choice: Choice::TerminalChainIoAdmission {
            core_hash: plan.into(),
            acceptance_hash: acceptance.into(),
        },
    })
}
impl DecisionReceipt {
    pub(in crate::engine::membership_transition) fn plan_terminal_chain_io(
        epoch: &str,
        identity: &MemberIdentity,
        plan: &str,
        acceptance: &str,
    ) -> Result<Self> {
        let record = record(epoch, identity, plan, acceptance)?;
        Ok(Self {
            record_hash: object_hash(DECISION_KIND, &record)?,
            record,
        })
    }
}
impl DecisionStore {
    pub(in crate::engine::membership_transition) fn terminal_chain_io_slot_present(
        disk: &DiskRoot,
        epoch: &str,
    ) -> Result<bool> {
        validate_hash(epoch)?;
        Ok(disk
            .optional_metadata(&decision_directory(disk, epoch))?
            .is_some())
    }
    pub(in crate::engine::membership_transition) fn terminal_chain_io_receipt(
        &self,
        plan: &str,
        acceptance: &str,
        install: bool,
    ) -> Result<DecisionReceipt> {
        let record = record(
            &self.binding.transition_hash,
            &self.binding.identity,
            plan,
            acceptance,
        )?;
        self.select(
            &record,
            install,
            #[cfg(feature = "fault-injection")]
            None,
        )
    }
}
