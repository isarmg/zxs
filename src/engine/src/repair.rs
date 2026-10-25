//! Repair the original unpublished candidate using complete verified bytes.
use crate::{disk::*, engine::Prepared, journal::Candidate, *};
use platform_io::{
    fs_safety::linux::{MountPolicy, OpenAt2Root},
    payload::{Expectation, verify_full},
};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn required(message: impl Into<String>) -> Error {
    Error::new(storage_contract::ErrorCode::RecoveryRequired, message)
}
fn matching(path: &Path, version: &FileVersion) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => {
            let message = format!("cannot inspect recovery bytes: {e}");
            return Err(Error::from(e).context(ErrorCode::RecoveryRequired, message));
        }
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
            return Err(required("recovery source is not a regular non-linked file"));
        }
        Ok(meta) if meta.len() != version.logical_size => return Ok(false),
        Ok(_) => {}
    }
    let mut file = open_regular(path)?;
    let hash = blake3::Hash::from_hex(&version.content_hash).map_err(|source| {
        Error::from(source).context(
            ErrorCode::RecoveryRequired,
            "candidate content digest is invalid",
        )
    })?;
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
            validate_seal(&self.config, disk)?;
            let directory = version_dir(disk, &version.history_tree_id, &version.version_id);
            let source = crate::container_storage::content_path(&directory, version)?;
            if matching(&source, version)? {
                return Ok(Some(source));
            }
        }
        for workspace in self.workspaces.values() {
            let directory = self.workspace_data_directory(workspace)?;
            if !directory.exists() {
                continue;
            }
            // Older generations remain valid recovery candidates even after
            // the current workspace manifest points at newer dirty bytes.
            for entry in fs::read_dir(&directory)? {
                let entry = entry?;
                if entry
                    .path()
                    .extension()
                    .is_some_and(|suffix| suffix == "full")
                    && matching(&entry.path(), version)?
                {
                    return Ok(Some(entry.path()));
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
                && matching(&source, version)?
            {
                return Ok(Some(source));
            }
        }
        for disk in &self.config.disks {
            validate_seal(&self.config, disk)?;
            let quarantine = disk.root.join("quarantine");
            if !quarantine.exists() {
                continue;
            }
            for entry in fs::read_dir(&quarantine)? {
                let entry = entry?;
                if !entry.file_type()?.is_dir()
                    || !entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(&format!("{}-repair-", version.version_id))
                {
                    continue;
                }
                // Quarantine is an inspection bundle, independently of the
                // live container layout used by the original version.
                let source = entry.path().join("content").join(&version.creation_name);
                if matching(&source, version)? {
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
                validate_seal(&self.config, disk)?;
                let directory = version_dir(disk, &version.history_tree_id, &version.version_id);
                let intact =
                    read_json::<FileVersion>(&directory.join("version.json")).is_ok_and(|stored| {
                        object_hash("version", &stored).ok() == object_hash("version", version).ok()
                    }) && verify_payload(&directory, version).is_ok();
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
                let quarantine = disk.root.join("quarantine");
                private_dir(&quarantine)?;
                let name = format!("{}-repair-{}", version.version_id, new_id("req"));
                let bundle = quarantine.join(&name);
                private_dir(&bundle)?;
                let content = crate::container_storage::content_path(&directory, version)?;
                match fs::symlink_metadata(&content) {
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                    Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
                        return Err(required("damaged payload is not a trusted regular file"));
                    }
                    Ok(_) => {
                        let destination = bundle.join("content");
                        private_dir(&destination)?;
                        let from = OpenAt2Root::open(
                            content.parent().expect("content parent"),
                            MountPolicy::SameMount,
                        )
                        .map_err(|source| {
                            let message = source.to_string();
                            Error::from(source).context(ErrorCode::RecoveryRequired, message)
                        })?;
                        let to = OpenAt2Root::open(&destination, MountPolicy::SameMount).map_err(
                            |source| {
                                let message = source.to_string();
                                Error::from(source).context(ErrorCode::RecoveryRequired, message)
                            },
                        )?;
                        rustix::fs::renameat_with(
                            from.directory(),
                            content.file_name().expect("payload name"),
                            to.directory(),
                            version.creation_name.as_str(),
                            rustix::fs::RenameFlags::NOREPLACE,
                        )
                        .map_err(std::io::Error::from)?;
                        from.directory().sync_all()?;
                        to.directory().sync_all()?;
                        if source == content {
                            source = destination.join(&version.creation_name);
                        }
                    }
                }
                match fs::symlink_metadata(&directory) {
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(e) => return Err(e.into()),
                    Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => {
                        return Err(required("damaged replica path is not a trusted directory"));
                    }
                    Ok(_) => {}
                }
                let parent = directory
                    .parent()
                    .ok_or_else(|| required("replica parent missing"))?;
                let from = OpenAt2Root::open(parent, MountPolicy::SameMount).map_err(|source| {
                    let message = source.to_string();
                    Error::from(source).context(ErrorCode::RecoveryRequired, message)
                })?;
                let to = OpenAt2Root::open(&bundle, MountPolicy::SameMount).map_err(|source| {
                    let message = source.to_string();
                    Error::from(source).context(ErrorCode::RecoveryRequired, message)
                })?;
                rustix::fs::renameat_with(
                    from.directory(),
                    directory.file_name().expect("version directory name"),
                    to.directory(),
                    "metadata",
                    rustix::fs::RenameFlags::NOREPLACE,
                )
                .map_err(std::io::Error::from)?;
                from.directory().sync_all()?;
                to.directory().sync_all()?;
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
