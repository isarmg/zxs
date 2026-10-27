//! Repair the original unpublished candidate using complete verified bytes.
use crate::{disk::*, engine::Prepared, journal::Candidate, *};
use platform_io::payload::{Expectation, verify_full};
use std::{fs::File, path::PathBuf};

fn required(message: impl Into<String>) -> Error {
    Error::new(storage_contract::ErrorCode::RecoveryRequired, message)
}
fn matching(source: Result<File>, version: &FileVersion) -> Result<bool> {
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
        Ok(file) => file,
    };
    if file.metadata()?.len() != version.logical_size {
        return Ok(false);
    }
    let hash = version.content_hash.digest();
    match verify_full(
        &mut file,
        Expectation {
            bytes: version.logical_size,
            blake3: Some(hash),
        },
    ) {
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
    ) -> Result<Option<PathBuf>> {
        for slot in &placement.route_slot_ids {
            let disk = self.disk(slot)?;
            let root = &self.disk_resources.get(disk)?.root;
            let directory = version_dir(disk, &version.history_tree_id, &version.version_id);
            let source =
                crate::container_storage::content_path_at(root, &directory, version, &mut |_| {
                    Ok(())
                })?;
            if matching(root.open_regular(&source), version)? {
                return Ok(Some(source));
            }
        }
        for workspace in self.workspaces.values() {
            let directory = self.workspace_data_directory(workspace)?;
            let root = self.planning_context().workspace_data_root(workspace)?;
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
                    && matching(root.open_regular(&path), version)?
                {
                    return Ok(Some(path));
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
                && matching(self.planning_context().open_source(&source), version)?
            {
                return Ok(Some(source));
            }
        }
        for disk in &self.config.disks {
            let root = &self.disk_resources.get(disk)?.root;
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
                if matching(root.open_regular(&source), version)? {
                    return Ok(Some(source));
                }
            }
        }
        Ok(None)
    }

    pub(crate) fn repair_pending_payloads(&self, candidate: &Candidate) -> Result<()> {
        for evidence in &candidate.versions {
            let version = &evidence.version;
            let mut damaged = Vec::new();
            for slot in &evidence.placement.route_slot_ids {
                let disk = self.disk(slot)?;
                let handle = self.disk_resources.get(disk)?;
                let directory = version_dir(disk, &version.history_tree_id, &version.version_id);
                let intact = handle
                    .root
                    .read_json_observed::<FileVersion>(&directory.join("version.json"), &mut |_| {
                        Ok(())
                    })
                    .is_ok_and(|stored| stored == *version)
                    && handle.verify_payload(&directory, version).is_ok();
                if !intact {
                    damaged.push((disk.clone(), directory));
                }
            }
            if damaged.is_empty() {
                continue;
            }
            let mut source = self
                .pending_payload_source(version, &evidence.placement)?
                .ok_or_else(|| {
                    required(format!(
                        "no verified complete bytes can repair {}; inspect permanent-loss recovery",
                        version.version_id
                    ))
                })?;
            for (disk, directory) in damaged {
                let root = &self.disk_resources.get(&disk)?.root;
                let quarantine = disk.root.join("quarantine");
                root.private_dir(&quarantine)?;
                let name = format!("{}-repair-{}", version.version_id, new_id("req"));
                let bundle = quarantine.join(&name);
                root.private_dir(&bundle)?;
                let content = crate::container_storage::content_path_at(
                    root,
                    &directory,
                    version,
                    &mut |_| Ok(()),
                )?;
                match root.open_regular(&content) {
                    Err(Error::Filesystem(platform_io::fs_safety::Error::Io(error)))
                    | Err(Error::Contract(storage_contract::Error::Io { source: error, .. }))
                        if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                    Ok(_) => {
                        let destination = bundle.join("content");
                        root.private_dir(&destination)?;
                        let from = root.directory(content.parent().expect("content parent"))?;
                        let to = root.directory(&destination)?;
                        rustix::fs::renameat_with(
                            &from,
                            content.file_name().expect("payload name"),
                            &to,
                            version.creation_name.as_str(),
                            rustix::fs::RenameFlags::NOREPLACE,
                        )
                        .map_err(std::io::Error::from)?;
                        from.sync_all()?;
                        to.sync_all()?;
                        if source == content {
                            source = destination.join(&version.creation_name);
                        }
                    }
                }
                match root.directory(&directory) {
                    Err(Error::Filesystem(platform_io::fs_safety::Error::Io(error)))
                    | Err(Error::Contract(storage_contract::Error::Io { source: error, .. }))
                        if error.kind() == std::io::ErrorKind::NotFound =>
                    {
                        continue;
                    }
                    Err(error) => return Err(error),
                    Ok(_) => {}
                }
                let parent = directory
                    .parent()
                    .ok_or_else(|| required("replica parent missing"))?;
                let from = root.directory(parent)?;
                let to = root.directory(&bundle)?;
                rustix::fs::renameat_with(
                    &from,
                    directory.file_name().expect("version directory name"),
                    &to,
                    "metadata",
                    rustix::fs::RenameFlags::NOREPLACE,
                )
                .map_err(std::io::Error::from)?;
                from.sync_all()?;
                to.sync_all()?;
            }
            self.prepare_payload(&Prepared {
                version: version.clone(),
                placement: evidence.placement.clone(),
                source,
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../test/engine/unit/repair/tests.rs"]
mod tests;
