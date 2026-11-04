use super::*;
use std::collections::BTreeSet;

impl Engine {
    /// The fence intentionally accepts a broader target. Pending enrollment is
    /// narrower and never rewrites an unsupported intent to make it admissible.
    pub(super) fn validate_pending_target(&self) -> Result<Vec<DiskConfig>> {
        let fence = self.membership_fence.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "pending members require original fence",
            )
        })?;
        let Origin::Genesis { config: origin } = &fence.intent.origin;
        let target = &fence.intent.target;
        if !same_settings(origin, target) || !same_settings(origin, &self.config) {
            return Err(unsupported());
        }
        let old = origin
            .disks
            .iter()
            .map(|disk| (disk.disk_id.as_str(), disk))
            .collect::<BTreeMap<_, _>>();
        let current = self
            .config
            .disks
            .iter()
            .map(|disk| (disk.disk_id.as_str(), disk))
            .collect::<BTreeMap<_, _>>();
        let next = target
            .disks
            .iter()
            .map(|disk| (disk.disk_id.as_str(), disk))
            .collect::<BTreeMap<_, _>>();
        if old.len() != current.len()
            || old.iter().any(|(id, disk)| {
                current.get(id).is_none_or(|other| !same_disk(disk, other))
                    || next.get(id).is_none_or(|other| !same_disk(disk, other))
            })
        {
            return Err(unsupported());
        }
        let old_slots = origin
            .disks
            .iter()
            .map(|disk| disk.slot_id.to_ascii_lowercase())
            .collect::<BTreeSet<_>>();
        let added = target
            .disks
            .iter()
            .filter(|disk| !old.contains_key(disk.disk_id.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if added.is_empty()
            || added.iter().any(|disk| {
                !disk.metadata_member
                    || disk.binding_generation != 1
                    || old_slots.contains(&disk.slot_id.to_ascii_lowercase())
            })
        {
            return Err(unsupported());
        }
        for disk in &target.disks {
            if disk.root.starts_with(&self.root) || self.root.starts_with(&disk.root) {
                return Err(Error::new(
                    ErrorCode::UnsafePath,
                    "pending target overlaps system directory",
                ));
            }
        }
        Ok(added)
    }

    pub(super) fn admit_pending_members(
        &self,
        source: PendingControlArchive,
        archive_root: &str,
        added: Vec<DiskConfig>,
        directories: Vec<PendingMemberDirectory>,
    ) -> Result<PendingMembership> {
        let fence = self
            .membership_fence
            .as_ref()
            .expect("invariant: pending target was admitted");
        let mut supplied = BTreeMap::new();
        for directory in directories {
            if supplied
                .insert(directory.disk_id, directory.directory)
                .is_some()
            {
                return Err(Error::new(
                    ErrorCode::ConfigInvalid,
                    "duplicate pending directory disk id",
                ));
            }
        }
        if supplied.len() != added.len() {
            return Err(Error::new(
                ErrorCode::ConfigInvalid,
                "pending directory set differs from additions",
            ));
        }
        let target_hash = object_hash("membership-target", &fence.intent.target)?;
        let mut identities = BTreeSet::new();
        for handle in self.disk_resources.handles() {
            let identity = verify_named_root(&handle.root)?;
            identities.insert((identity.device, identity.inode));
        }
        let system = DiskRoot::open(&self.root)?;
        let identity = FileIdentity::from_fd(system.directory(system.path())?)?;
        identities.insert((identity.device, identity.inode));
        let identity = source.identity()?;
        identities.insert((identity.device, identity.inode));
        let mut roots = Vec::new();
        // Every target is inspected before even creating new lock files.
        for disk in added {
            let directory = supplied.remove(&disk.disk_id).ok_or_else(|| {
                Error::new(
                    ErrorCode::ConfigInvalid,
                    "missing newly added member directory",
                )
            })?;
            let root = DiskRoot::from_directory(disk.root.clone(), directory)?;
            let identity = FileIdentity::from_fd(root.directory(root.path())?)?;
            if !identities.insert((identity.device, identity.inode)) {
                return Err(Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "pending root aliases an admitted directory",
                ));
            }
            let seal = PendingDiskSeal {
                format: PendingFormat::MembershipPending,
                identity: member_identity(&fence.intent.target, &disk, identity),
                transition_hash: fence.hash.clone(),
                target_config_hash: target_hash.clone(),
                control_archive_root: archive_root.to_owned(),
            };
            let bound = inspect_new_root(&root, &seal, false)?;
            roots.push((disk, root, seal, bound));
        }
        let mut resources = BTreeMap::new();
        for (disk, root, seal, bound) in roots {
            let lock = root.lock_file()?;
            platform_io::fs_safety::linux::SingleLinkRequirement::check(&lock).map_err(fs_error)?;
            rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive)
                .map_err(|error| {
                    Error::from(std::io::Error::from(error)).context(
                        ErrorCode::WriterAlreadyActive,
                        "another writer owns pending member",
                    )
                })?;
            verify_named_root(&root)?;
            let now_bound = inspect_new_root(&root, &seal, true)?;
            if bound != now_bound {
                return Err(Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "pending root binding changed during admission",
                ));
            }
            resources.insert(
                disk.disk_id,
                MemberResource::Added(Box::new(PendingDisk {
                    root,
                    seal,
                    _lock: lock,
                })),
            );
        }
        let mut members = Vec::new();
        for disk in fence.intent.target.metadata_members() {
            let resource = match resources.remove(&disk.disk_id) {
                Some(resource) => resource,
                None => MemberResource::Retained(Arc::clone(self.disk_resources.get(disk)?)),
            };
            let physical = FileIdentity::from_fd(resource.root().directory(&disk.root)?)?;
            let origin = match &resource {
                MemberResource::Retained(handle) => EnrollmentOrigin::Retained {
                    original_seal_hash: object_hash("membership-original-seal", &handle.seal)?,
                },
                MemberResource::Added(handle) => EnrollmentOrigin::Added {
                    pending_seal_hash: object_hash(PENDING_SEAL_KIND, &handle.seal)?,
                },
            };
            let enrollment = MemberEnrollment {
                scope: ControlScope::ControlMetadataOnly,
                identity: member_identity(&fence.intent.target, disk, physical),
                origin,
                transition_hash: fence.hash.clone(),
                target_config_hash: target_hash.clone(),
                control_archive_root: archive_root.to_owned(),
            };
            let bootstrap = PreparedMetadataObject::new(
                BOOTSTRAP_KIND,
                &ControlBootstrap {
                    scope: ControlScope::ControlMetadataOnly,
                    identity: enrollment.identity.clone(),
                    enrollment_hash: object_hash(ENROLLMENT_KIND, &enrollment)?,
                    transition_hash: fence.hash.clone(),
                    target_config_hash: target_hash.clone(),
                    control_archive_root: archive_root.to_owned(),
                    frozen_boundary: match &fence.intent.boundary {
                        None => FrozenBoundary::EmptyGenesis,
                        Some(boundary) => FrozenBoundary::Published {
                            boundary: boundary.clone(),
                        },
                    },
                    fence_sequence: fence.intent.fence_sequence,
                },
                MAX_METADATA,
            )?;
            members.push(PendingMember {
                disk: disk.clone(),
                resource,
                enrollment,
                bootstrap,
            });
        }
        // Keep stable observable progress, independent of a caller's config order.
        members.sort_by(|a, b| a.disk.disk_id.cmp(&b.disk.disk_id));
        Ok(PendingMembership {
            source,
            transition_hash: fence.hash.clone(),
            archive_root: archive_root.to_owned(),
            members,
            receipts: BTreeMap::new(),
            joint: None,
            #[cfg(feature = "fault-injection")]
            journal_fault: None,
            #[cfg(feature = "fault-injection")]
            joint_fault: None,
        })
    }

    pub(super) fn validate_pending_resource(&self, member: &PendingMember) -> Result<()> {
        verify_named_root(member.resource.root())?;
        match &member.resource {
            MemberResource::Retained(handle) => {
                let seal = decode_seal(
                    &handle
                        .root
                        .read_bytes(&member.disk.root.join("disk.json"), MAX_METADATA)?,
                )?;
                validate_stored_seal(&seal, &handle.seal)
            }
            MemberResource::Added(handle) => {
                inspect_new_root(&handle.root, &handle.seal, true).map(|_| ())
            }
        }
    }
}

fn inspect_new_root(root: &DiskRoot, expected: &PendingDiskSeal, own_lock: bool) -> Result<bool> {
    let names = root
        .entries(root.path())?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(fs_error)?;
    let stored = root.open_optional_regular(&root.path().join("disk.json"))?;
    match stored {
        None => {
            // Only a lock created by this still-live admission can be unbound.
            // A restart cannot adopt a lock or atomic scratch left before seal.
            if names
                .iter()
                .any(|name| !own_lock || name.as_os_str() != ".writer.lock")
            {
                return Err(Error::new(
                    ErrorCode::DiskAlreadyEnrolled,
                    "unbound pending root is nonempty; preserve and inspect existing material",
                ));
            }
            Ok(false)
        }
        Some(file) => {
            let actual: PendingDiskSeal = decode_strict(&read_file_bytes(file, MAX_METADATA)?)?;
            if &actual != expected {
                return Err(Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "pending disk seal differs from held member or transition",
                ));
            }
            for name in names {
                if platform_io::fs_safety::AtomicFile::is_temporary_name(&name) {
                    root.open_regular(&root.path().join(name.as_path()))?;
                } else if !matches!(
                    name.as_os_str().to_str(),
                    Some("disk.json" | ".writer.lock" | "metadata")
                ) {
                    return Err(Error::new(
                        ErrorCode::UnsafePath,
                        "pending root contains active or unrelated material",
                    ));
                }
            }
            validate_metadata_namespace(root, &expected.transition_hash)?;
            Ok(true)
        }
    }
}
fn validate_metadata_namespace(root: &DiskRoot, transition: &str) -> Result<()> {
    let metadata = root.path().join("metadata");
    if root.optional_metadata(&metadata)?.is_none() {
        return Ok(());
    }
    for name in root.entries(&metadata)? {
        if name.map_err(fs_error)?.as_os_str() != "membership-pending" {
            return Err(Error::new(
                ErrorCode::UnsafePath,
                "pending disk has an active metadata namespace",
            ));
        }
    }
    let pending = metadata.join("membership-pending");
    if root.optional_metadata(&pending)?.is_none() {
        return Ok(());
    }
    for name in root.entries(&pending)? {
        if name.map_err(fs_error)?.as_os_str() != transition {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "pending disk has a different transition namespace",
            ));
        }
    }
    Ok(())
}
pub(super) fn verify_named_root(root: &DiskRoot) -> Result<FileIdentity> {
    let named = DiskRoot::open(root.path())?;
    let held = FileIdentity::from_fd(root.directory(root.path())?)?;
    if held != FileIdentity::from_fd(named.directory(named.path())?)? {
        return Err(Error::new(
            ErrorCode::DiskIdentityMismatch,
            "pending member root moved after admission",
        ));
    }
    Ok(held)
}
pub(super) fn member_identity(
    config: &ValidatedConfig,
    disk: &DiskConfig,
    physical: FileIdentity,
) -> MemberIdentity {
    MemberIdentity {
        pool_id: config.pool_id.clone(),
        disk_id: disk.disk_id.clone(),
        slot_id: disk.slot_id.clone(),
        binding_generation: disk.binding_generation,
        device: physical.device,
        inode: physical.inode,
        simulation: config.mode == Mode::Development,
    }
}
fn unsupported() -> Error {
    Error::new(
        ErrorCode::ConfigConflict,
        "pending enrollment supports only unchanged original members plus fresh generation-one metadata members",
    )
}
pub(super) fn same_disk(old: &DiskConfig, new: &DiskConfig) -> bool {
    let DiskConfig {
        slot_id,
        disk_id,
        root,
        level,
        upstream_slot_id,
        metadata_member,
        binding_generation,
        capacity_bytes,
    } = old;
    slot_id == &new.slot_id
        && disk_id == &new.disk_id
        && root == &new.root
        && *level == new.level
        && upstream_slot_id == &new.upstream_slot_id
        && *metadata_member == new.metadata_member
        && *binding_generation == new.binding_generation
        && *capacity_bytes == new.capacity_bytes
}
pub(super) fn same_settings(old: &Config, new: &Config) -> bool {
    let Config {
        pool_id,
        mode,
        disks: _,
        checksum_chunk_bytes,
        max_file_bytes,
        max_workspace_bytes,
        max_workspaces,
        max_retained_workspaces,
        max_retained_workspaces_per_principal,
        max_retained_workspace_metadata_bytes,
        max_retained_workspace_metadata_bytes_per_principal,
        max_entries,
        metadata_reserve_bytes,
        workspace_ttl_seconds,
    } = old;
    pool_id == &new.pool_id
        && mode == &new.mode
        && *checksum_chunk_bytes == new.checksum_chunk_bytes
        && *max_file_bytes == new.max_file_bytes
        && *max_workspace_bytes == new.max_workspace_bytes
        && *max_workspaces == new.max_workspaces
        && *max_retained_workspaces == new.max_retained_workspaces
        && *max_retained_workspaces_per_principal == new.max_retained_workspaces_per_principal
        && *max_retained_workspace_metadata_bytes == new.max_retained_workspace_metadata_bytes
        && *max_retained_workspace_metadata_bytes_per_principal
            == new.max_retained_workspace_metadata_bytes_per_principal
        && *max_entries == new.max_entries
        && *metadata_reserve_bytes == new.metadata_reserve_bytes
        && *workspace_ttl_seconds == new.workspace_ttl_seconds
}
