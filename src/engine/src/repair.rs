//! Repair the original unpublished candidate using complete verified bytes.
use crate::{
    disk::*,
    engine::ReadSource,
    journal::Candidate,
    *,
};
use platform_io::payload::Expectation;

fn matching(source: Result<impl Into<ReadFile>>, version: &FileVersion) -> Result<bool> {
    let mut file = match source {
        Err(Error::Filesystem(platform_io::fs_safety::Error::Io(error)))
        | Err(Error::Contract(storage_contract::Error::Io { source: error, .. }))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            return Ok(false);
        }
        Err(error) => {
            return Err(error.context(ErrorCode::RecoveryRequired, "cannot open recovery bytes"));
        }
        Ok(file) => file.into(),
    };
    if file.metadata()?.len() != version.logical_size {
        return Ok(false);
    }
    let hash = version.content_hash.digest();
    match file.verify_full(Expectation {
        bytes: version.logical_size,
        blake3: Some(hash),
    }) {
        Ok(_) => Ok(true),
        Err(platform_io::payload::Error::Digest | platform_io::payload::Error::Length { .. }) => {
            Ok(false)
        }
        Err(e) => {
            let message = format!("cannot verify recovery bytes: {e}");
            Err(Error::from(e).context(ErrorCode::RecoveryRequired, message))
        }
    }
}
impl Engine {
    /// The same source discovery is used to reject a permanent-loss declaration
    /// while known complete bytes can still repair the frozen candidate.
    pub(crate) fn pending_payload_source(
        &self,
        version: &FileVersion,
        placement: &Placement,
        sources: &crate::container_storage::PreparedContainerEntities,
    ) -> Result<Option<ReadSource>> {
        for slot in &placement.route_slot_ids {
            let disk = self.disk(slot)?;
            let scope = sources.retain_scope(version.file_id.parse()?, disk.disk_id.parse()?)?;
            let source = ReadSource::container(scope, version)?;
            if matching(source.open(), version)? {
                return Ok(Some(source));
            }
        }
        for id in self.retained_workspace_ids() {
            let workspace = self.workspace_recovery_hint(id)?;
            let directory = self.workspace_data_directory(&workspace)?;
            let context = self.planning_context();
            let owner = context.workspace_disk(&workspace)?;
            let root = &owner.root;
            let entries = match root.entries(&directory) {
                Ok(entries) => entries,
                Err(Error::Filesystem(platform_io::fs_safety::Error::Io(error)))
                | Err(Error::Contract(storage_contract::Error::Io { source: error, .. }))
                    if error.kind() == std::io::ErrorKind::NotFound =>
                {
                    continue;
                }
                Err(error) => return Err(error),
            };
            // Older generations remain valid recovery candidates even after
            // the current workspace manifest points at newer dirty bytes.
            for entry in entries {
                let entry = entry?;
                let path = directory.join(entry.as_path());
                if path.extension().is_some_and(|suffix| suffix == "full")
                    && matching(root.open_leased_regular(&path), version)?
                {
                    return Ok(Some(ReadSource::on_disk(owner, path)));
                }
            }
        }
        for published in self.state.versions.values().filter(|v| {
            v.content_hash == version.content_hash && v.logical_size == version.logical_size
        }) {
            if self
                .state
                .content_states
                .get(&published.version_id)
                .is_some_and(|s| *s != ContentState::Available)
            {
                continue;
            }
            if let Ok((source, _)) = self.valid_replica(&published.version_id)
                && matching(source.open(), version)?
            {
                return Ok(Some(source));
            }
        }
        for disk in &self.config.disks {
            let owner = self.disk_resources.get(disk)?;
            let root = &owner.root;
            let quarantine = disk.root.join("quarantine");
            let entries = match root.entries(&quarantine) {
                Ok(entries) => entries,
                Err(Error::Filesystem(platform_io::fs_safety::Error::Io(error)))
                | Err(Error::Contract(storage_contract::Error::Io { source: error, .. }))
                    if error.kind() == std::io::ErrorKind::NotFound =>
                {
                    continue;
                }
                Err(error) => return Err(error),
            };
            for entry in entries {
                let entry = entry?;
                let path = quarantine.join(entry.as_path());
                if !root
                    .optional_metadata(&path)?
                    .is_some_and(|metadata| metadata.is_dir())
                    || !entry
                        .as_os_str()
                        .to_string_lossy()
                        .starts_with(&format!("{}-repair-", version.version_id))
                {
                    continue;
                }
                // Quarantine is an inspection bundle, independently of the
                // live container layout used by the original version.
                let source = path.join("content").join(&version.creation_name);
                if matching(root.open_leased_regular(&source), version)? {
                    return Ok(Some(ReadSource::on_disk(owner, source)));
                }
            }
        }
        Ok(None)
    }

    pub(crate) fn repair_pending_payloads(&self, candidate: &Candidate) -> Result<()> {
        self.verify_candidate_membership(candidate)?;
        let original = crate::publication_evidence::verified_recovery_snapshot_at(
            &self.disk_resources, &self.members(), &candidate.snapshot_hash,
        )?;
        let sources = crate::container_storage::PreparedContainerEntities::observe_candidate(
            &self.planning_context(), candidate, &original,
        )?;
        for evidence in &candidate.versions {
            let version = &evidence.version;
            let mut damaged = Vec::new();
            for slot in &evidence.placement.route_slot_ids {
                let disk = self.disk(slot)?;
                let scope = sources.scope(version.file_id.parse()?, disk.disk_id.parse()?)?;
                let material = VersionMaterial::current(scope, version)?;
                let directory = material.directory()?;
                let intact = material.root()?
                    .read_json_observed::<FileVersion>(&directory.join("version.json"), &mut |_| {
                        Ok(())
                    })
                    .is_ok_and(|stored| stored == *version)
                    && material.verify_payload().is_ok();
                if !intact {
                    damaged.push((disk.clone(), directory));
                }
            }
            if damaged.is_empty() {
                continue;
            }
            // The legacy repair moves both BODY and metadata into disk-global
            // quarantine with source-first durability/error ordering. Neither
            // its raw disk path nor the destination-first held rename is that
            // transaction. Preserve observation, but perform no partial repair.
            return Err(crate::engine::FreshLayoutError::SourceQuarantine.into());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../test/engine/unit/repair/tests.rs"]
mod tests;
