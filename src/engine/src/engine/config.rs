//! Disk topology, resource admission configuration and OS writer locks.
use crate::{disk::*, *};
use std::{collections::BTreeSet, fs::File, path::Path};

pub(super) fn acquire_locks(
    root: &Path,
    config: &Config,
    allow_missing: bool,
) -> Result<Vec<File>> {
    let mut paths = vec![root.join("writer.lock")];
    for d in &config.disks {
        if allow_missing {
            if disk_unavailable(d)? {
                continue;
            }
            validate_seal(config, d)?;
            paths.push(d.root.join(".writer.lock"));
        } else if d.root.try_exists()? {
            paths.push(d.root.join(".writer.lock"));
        }
    }
    let mut locks = Vec::new();
    for path in paths {
        let f = File::from(
            rustix::fs::open(
                &path,
                rustix::fs::OFlags::CREATE
                    | rustix::fs::OFlags::RDWR
                    | rustix::fs::OFlags::NOFOLLOW
                    | rustix::fs::OFlags::NONBLOCK
                    | rustix::fs::OFlags::CLOEXEC,
                rustix::fs::Mode::from_raw_mode(0o600),
            )
            .map_err(std::io::Error::from)?,
        );
        platform_io::fs_safety::linux::SingleLinkRequirement::check(&f).map_err(|e| {
            let message = e.to_string();
            Error::from(e).context(storage_contract::ErrorCode::UnsafePath, message)
        })?;
        rustix::fs::flock(&f, rustix::fs::FlockOperation::NonBlockingLockExclusive).map_err(
            |source| {
                Error::from(std::io::Error::from(source)).context(
                    storage_contract::ErrorCode::WriterAlreadyActive,
                    "another process owns this pool or disk",
                )
            },
        )?;
        locks.push(f);
    }
    Ok(locks)
}
pub(super) fn validate_config(c: &Config) -> Result<()> {
    validate_id(&c.pool_id, "pool")?;
    if !c.disks.iter().any(|d| d.metadata_member)
        || c.max_file_bytes == 0
        || c.max_workspace_bytes == 0
        || c.max_file_bytes > c.max_workspace_bytes
        || c.max_workspaces == 0
        || c.max_retained_workspaces == 0
        || c.max_retained_workspaces_per_principal == 0
        || c.max_retained_workspace_metadata_bytes == 0
        || c.max_retained_workspace_metadata_bytes_per_principal == 0
        || c.max_entries == 0
        || !SUPPORTED_CHECKSUM_CHUNK_BYTES.contains(&c.checksum_chunk_bytes)
    {
        return Err(Error::new(
            storage_contract::ErrorCode::ConfigInvalid,
            "invalid metadata membership, resource limits or checksum size",
        ));
    }
    let chunks = c.max_file_bytes.div_ceil(c.checksum_chunk_bytes as u64);
    if chunks > (MAX_CHECKSUM_MANIFEST as u64 - CHECKSUM_HEADER_BYTES as u64) / 32 {
        return Err(Error::new(
            storage_contract::ErrorCode::ConfigInvalid,
            "file limit exceeds the bounded checksum manifest capacity",
        ));
    }
    let mut slots = BTreeSet::new();
    let mut ids = BTreeSet::new();
    let mut roots = BTreeSet::new();
    let mut successors = BTreeSet::new();
    for d in &c.disks {
        name_policy::validate_slot(&d.slot_id)?;
        validate_id(&d.disk_id, "disk")?;
        if !d.root.is_absolute()
            || d.root
                .components()
                .any(|part| part == std::path::Component::ParentDir)
        {
            return Err(Error::new(
                storage_contract::ErrorCode::ConfigInvalid,
                "disk roots must be absolute and contain no parent traversal",
            ));
        }
        if !slots.insert(d.slot_id.to_ascii_lowercase())
            || !ids.insert(&d.disk_id)
            || !roots.insert(&d.root)
            || d.level == 0
            || d.binding_generation == 0
        {
            return Err(Error::new(
                storage_contract::ErrorCode::ConfigInvalid,
                "duplicate disk, path or slot identity",
            ));
        }
        if d.level == 1 && d.upstream_slot_id.is_some() {
            return Err(Error::new(
                storage_contract::ErrorCode::TopologyInvalid,
                "primary cannot have an upstream",
            ));
        }
        if d.level > 1 {
            let up = d.upstream_slot_id.as_ref().ok_or_else(|| {
                Error::new(
                    storage_contract::ErrorCode::TopologyInvalid,
                    "backup lacks upstream",
                )
            })?;
            let source = c
                .disks
                .iter()
                .find(|s| s.slot_id.eq_ignore_ascii_case(up))
                .ok_or_else(|| {
                    Error::new(
                        storage_contract::ErrorCode::TopologyInvalid,
                        "upstream slot missing",
                    )
                })?;
            if source.level.checked_add(1) != Some(d.level)
                || !successors.insert(up.to_ascii_lowercase())
            {
                return Err(Error::new(
                    storage_contract::ErrorCode::TopologyInvalid,
                    "route skips a level or source has multiple successors",
                ));
            }
        }
    }
    for (i, disk) in c.disks.iter().enumerate() {
        if c.disks
            .iter()
            .skip(i + 1)
            .any(|other| disk.root.starts_with(&other.root) || other.root.starts_with(&disk.root))
        {
            return Err(Error::new(
                storage_contract::ErrorCode::ConfigInvalid,
                "disk roots must not overlap",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/config/tests.rs"]
mod tests;
