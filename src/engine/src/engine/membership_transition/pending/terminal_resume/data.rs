//! Nonvoting terminal additions use the Add readiness format and held resources.
//! Their exact enrollment is derived from the certified request, not a vote or
//! an independently mutable second identity map.
use super::super::data::{self, DataDependency};
use super::*;

fn dependency(
    intent: &TransitionIntent,
    disk: &DiskConfig,
    archive: &str,
) -> Result<DataDependency> {
    let identity = intent.added_roots.get(&disk.disk_id).ok_or_else(|| {
        Error::new(
            ErrorCode::DiskIdentityMismatch,
            "resume data directory lacks its exact selected identity",
        )
    })?;
    let dependency = DataDependency::new(PendingDiskSeal {
        format: PendingFormat::MembershipPending,
        identity: identity.clone(),
        transition_hash: object_hash(INTENT_KIND, intent)?,
        target_config_hash: object_hash("membership-target", &intent.target)?,
        control_archive_root: archive.to_owned(),
    });
    dependency.verify(disk, intent, archive)?;
    Ok(dependency)
}

impl TerminalMetadataResume {
    pub(super) fn synchronize_data(&mut self) -> Result<()> {
        for disk in self.proposal.intent.target.disks.iter().filter(|disk| {
            !disk.metadata_member && self.proposal.intent.added_roots.contains_key(&disk.disk_id)
        }) {
            let resource = &self.members[&disk.disk_id];
            let dependency = dependency(
                &self.proposal.intent,
                disk,
                self.proposal.terminal.archive_root_hash(),
            )?;
            reservation::verify_writer(&resource.root, &resource._lock)?;
            admission::inspect_new_root(
                &resource.root,
                &resource.seal,
                &resource.reservation_name,
            )?;
            resource
                .root
                .write_json(&disk.root.join("disk.json"), &resource.seal, false)?;
            data::synchronize_resource(disk, resource, &dependency)?;
            self.ready_data_disks.insert(disk.disk_id.clone());
        }
        Ok(())
    }

    pub(super) fn verify_data_ready(&self) -> Result<()> {
        for disk in self.proposal.intent.target.disks.iter().filter(|disk| {
            !disk.metadata_member && self.proposal.intent.added_roots.contains_key(&disk.disk_id)
        }) {
            let dependency = dependency(
                &self.proposal.intent,
                disk,
                self.proposal.terminal.archive_root_hash(),
            )?;
            data::verify_ready_resource(&self.members[&disk.disk_id], &dependency)?;
        }
        Ok(())
    }
}

impl ResumeCore {
    /// The immutable proposal binds the complete data-ready seal, including the
    /// original request, selected physical root, target roles and old archive.
    /// No new serialization field changes an existing metadata-only core hash.
    pub(in crate::engine::membership_transition::pending) fn data_dependency(
        &self,
        disk_id: &str,
    ) -> Result<Option<DataDependency>> {
        let intent = self.intent()?;
        let disk = intent.target.disks.iter().find(|disk| {
            disk.disk_id == disk_id
                && !disk.metadata_member
                && intent.added_roots.contains_key(disk_id)
        });
        disk.map(|disk| dependency(&intent, disk, self.terminal().archive_root_hash()))
            .transpose()
    }

    /// Recheck real evidence before either a live handle handoff or cold
    /// resource acquisition can promote this exact fresh directory.
    pub(in crate::engine::membership_transition::pending) fn verify_selected_root(
        &self,
        root: &DiskRoot,
        disk: &DiskConfig,
    ) -> Result<()> {
        if disk.metadata_member {
            return self.verify_member(root, disk);
        }
        self.data_dependency(&disk.disk_id)?
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "resume data directory is outside the exact selected additions",
                )
            })?
            .verify_local(root)
    }
}
