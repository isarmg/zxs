//! Explicit activation and current-publication repair of physical main layouts.
use super::Engine;
use crate::{DirectoryLayoutReceipt, Error, ErrorCode, Result};
use std::collections::BTreeSet;

impl Engine {
    /// Reconcile an initialized directory forest under this Engine's writer
    /// fences. This changes only derived placement, not user Git commits. The
    /// stable main's saved working tree supplies names, including uncommitted
    /// saves; each initialized child retains its own independent main layout.
    pub fn reconcile_directory_layout(
        &mut self,
        repository_id: &str,
    ) -> Result<DirectoryLayoutReceipt> {
        self.require_write()?;
        storage_contract::validate_id(repository_id, "repo")?;
        if !self.state.repositories.contains_key(repository_id) {
            return Err(Error::new(
                ErrorCode::RepositoryNotFound,
                "directory layout repository is absent",
            ));
        }
        if !self
            .state
            .directory_git_repositories
            .values()
            .any(|git| git.repository_id == repository_id)
        {
            return Err(crate::DirectoryLayoutError::RootNotInitialized {
                directory_id: repository_id.to_owned(),
            }
            .into());
        }
        for disk in self.config.io_disks() {
            if self.disk_resources.available(disk).is_some() {
                super::provenance::validate_owned_seal(
                    &self.disk_resources,
                    disk,
                    &self.metadata_integrity,
                )?;
            }
        }
        // The immutable published State, admitted resource set and retained
        // writer locks cannot change while this &mut Engine operation runs.
        let selected = BTreeSet::from([repository_id.to_owned()]);
        let names = crate::container_storage::prepare_layout_repair(
            &self.config,
            &self.disk_resources,
            &self.state,
            Some(&selected),
            crate::database_install::InstallAdmission::NewOperation,
        )?;
        self.blocked = true;
        #[cfg(feature = "fault-injection")]
        let applied = names.apply_observed(&mut |point| self.failpoint(point));
        #[cfg(not(feature = "fault-injection"))]
        let applied = names.apply_with_receipt();
        match applied {
            Ok(receipt) => {
                self.blocked = false;
                Ok(receipt)
            }
            Err(error) => {
                self.metadata_integrity.invalidate();
                Err(error
                    .context(
                        ErrorCode::RecoveryRequired,
                        "physical directory layout repair was interrupted; recover before writing",
                    )
                    .retryable())
            }
        }
    }
}
