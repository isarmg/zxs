//! Typed control preparation for future lifecycle effects. No effect here changes
//! the live registry, visibility, I/O permissions, or historical topology.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "kebab-case")]
pub enum MembershipExitAction {
    PlannedOffline,
    Deregister,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct ExitEffect {
    action: MembershipExitAction,
    disk_id: String,
    slot_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    binding_generation: u64,
}
impl ExitEffect {
    pub(super) fn validate(
        &self,
        original: &ValidatedConfig,
        target: &ValidatedConfig,
    ) -> Result<()> {
        let affected = original
            .disks
            .iter()
            .find(|disk| disk.disk_id == self.disk_id)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::ConfigConflict,
                    "exit effect names no registered binding",
                )
            })?;
        if affected.slot_id != self.slot_id
            || affected.binding_generation != self.binding_generation
            || !pending::admission::same_settings(original, target)
            || original.disks.len() != target.disks.len()
        {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "exit effect changes its binding or pool settings",
            ));
        }
        for old in &original.disks {
            let mut expected = old.clone();
            if old.disk_id == self.disk_id {
                expected.metadata_member = false;
            }
            if !target
                .disks
                .iter()
                .any(|disk| pending::admission::same_disk(&expected, disk))
            {
                return Err(Error::new(
                    ErrorCode::ConfigConflict,
                    "exit preparation must preserve all historical topology records",
                ));
            }
        }
        Ok(())
    }
}

impl Engine {
    /// Prepare a lifecycle control decision, not a completed offline/delete action.
    /// The live registration and every historical route remain intact and business
    /// stays fenced. Lifecycle activation needs the separate registry/I/O protocol.
    pub fn begin_membership_exit_preparation(
        &mut self,
        disk_id: &str,
        action: MembershipExitAction,
        principal: &str,
        operation_id: OperationId,
    ) -> Result<MembershipFenceStatus> {
        let mut target = (*self.config).clone();
        let disk = target
            .disks
            .iter_mut()
            .find(|disk| disk.disk_id == disk_id)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::ConfigConflict,
                    "exit preparation names no current disk",
                )
            })?;
        let effect = ExitEffect {
            action,
            disk_id: disk.disk_id.clone(),
            slot_id: disk.slot_id.clone(),
            binding_generation: disk.binding_generation,
        };
        disk.metadata_member = false;
        if !target.disks.iter().any(|disk| disk.metadata_member) {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "terminal pool authority is not implemented; no exit fence was written",
            ));
        }
        self.begin_membership_handoff_with_effect(target, Some(effect), principal, operation_id)
    }
}

pub(super) fn validate_transition_target(
    original: &ValidatedConfig,
    target: &ValidatedConfig,
    effect: Option<&ExitEffect>,
) -> Result<()> {
    if let Some(effect) = effect {
        return effect.validate(original, target);
    }
    if target.pool_id != original.pool_id || !changes_registered_membership(original, target) {
        return Err(Error::new(
            ErrorCode::ConfigConflict,
            "handoff must retain the pool identity and change voters or add registered devices",
        ));
    }
    Ok(())
}
