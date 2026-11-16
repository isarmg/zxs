//! Ordinary Add preserves the initial namespace but changes its real epoch.
//! Only its exact published activation can authorize later automatic geometry.
use crate::engine::membership_transition::pending::activation::{
    Generation, authority_config_matches,
};
use crate::{AutomaticBackupError, Error, Result, State, ValidatedConfig};

impl ValidatedConfig {
    /// This is a read-only source gate. Callers still need their actual write
    /// admission, complete operation replay and held registered-root proofs.
    pub(crate) fn require_initial_backup_source(&self, source: &State) -> Result<()> {
        let Some(generation) = self.generation() else {
            return Ok(());
        };
        generation.require_initial_backup_source(self, source)
    }
}

impl Generation {
    fn require_initial_backup_source(
        &self, config: &ValidatedConfig, source: &State,
    ) -> Result<()> {
        if !self.portable.is_initial_addition_chain() {
            return Err(Error::AutomaticBackup(AutomaticBackupError::InitialOnly));
        }
        let expected = self.result()?;
        if !authority_config_matches(config, &self.intent.target)
            || source.sequence < expected.sequence
            || source.operations.get(&expected.operation_id) != Some(&expected)
            || source.operation_principals.get(&expected.operation_id)
                != Some(&self.intent.principal)
        {
            return Err(Error::AutomaticBackup(AutomaticBackupError::Source));
        }
        Ok(())
    }
}
