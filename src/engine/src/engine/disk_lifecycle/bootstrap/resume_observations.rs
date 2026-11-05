//! Reconstruct locator observations from an authenticated terminal request. These
//! are historical selected identities, not permission to skip live admission.
use super::*;
use crate::engine::membership_transition::{
    terminal::VerifiedTerminalAuthority, terminal_resume::ResumeCore,
};

impl RootObservation {
    pub(in crate::engine) fn from_terminal_resume(
        terminal: &VerifiedTerminalAuthority,
        core: &ResumeCore,
    ) -> crate::Result<Vec<Self>> {
        core.authenticated_roots(terminal)?
            .into_iter()
            .map(|(disk, physical)| {
                let record = RootRecord {
                    disk_id: disk.disk_id,
                    slot_id: disk.slot_id,
                    binding_generation: disk.binding_generation,
                    path: canonical_root(&disk.root).map_err(runtime::management_error)?,
                    device: physical.device,
                    inode: physical.inode,
                    metadata_member: disk.metadata_member,
                };
                record.validate().map_err(runtime::management_error)?;
                Ok(Self {
                    pool_id: terminal.authority.original().pool_id.clone(),
                    record,
                })
            })
            .collect()
    }
}
