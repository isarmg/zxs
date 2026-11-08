use super::*;
use std::collections::BTreeSet;

impl Engine {
    /// Revalidate persisted intent against its exact original configuration.
    /// Unsupported restored intents remain fenced; they are never rewritten.
    pub(super) fn validate_pending_target(&self) -> Result<Vec<DiskConfig>> {
        let fence = self.membership_fence.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "pending members require original fence",
            )
        })?;
        if matches!(&fence.intent.origin, Origin::BackupCutover { .. }) {
            return Err(Error::new(
                ErrorCode::UnsupportedOperation,
                "backup source fencing does not admit target members or ordinary Joint activation",
            ));
        }
        reservation::verify_intent_roots(&fence.intent)?;
        let origin = fence.intent.origin.config();
        let target = &fence.intent.target;
        if let Some(effect) = &fence.intent.effect {
            effect.validate(origin, target)?;
            if effect.is_terminal() {
                return Err(Error::new(
                    ErrorCode::ConfigConflict,
                    "terminal management uses its independent system authority, not pending active members",
                ));
            }
            if !same_settings(origin, &self.config)
                || origin.disks.len() != self.config.disks.len()
                || origin.disks.iter().any(|old| {
                    !self
                        .config
                        .disks
                        .iter()
                        .any(|current| same_disk(old, current))
                })
            {
                return Err(unsupported());
            }
            return Ok(Vec::new());
        }
        if !same_settings(origin, &self.config)
            || origin.disks.len() != self.config.disks.len()
            || origin.disks.iter().any(|old| {
                !self
                    .config
                    .disks
                    .iter()
                    .any(|current| same_disk(old, current))
            })
        {
            return Err(unsupported());
        }
        let added = validate_pure_add_target(origin, target, &self.root)?;
        self.require_addition_chain_registry(target)?;
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
        // Check every supplied FD against the pre-fence identity before
        // consuming held preparation or reacquiring its existing writer lock.
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
            let identity = member_identity(&fence.intent.target, &disk, identity);
            if fence.intent.added_roots.get(&disk.disk_id) != Some(&identity) {
                return Err(Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "pending root differs from the pre-fence held identity",
                ));
            }
            let reservation_name = reservation::name(
                &fence.intent.origin,
                &fence.intent.target,
                &fence.intent.principal,
                fence.intent.operation_id,
                &identity,
            )?;
            let seal = PendingDiskSeal {
                format: PendingFormat::MembershipPending,
                identity,
                transition_hash: fence.hash.clone(),
                target_config_hash: target_hash.clone(),
                control_archive_root: archive_root.to_owned(),
            };
            inspect_new_root(&root, &seal, &reservation_name)?;
            roots.push((disk, root, seal, reservation_name));
        }
        let mut resources = BTreeMap::new();
        for (disk, root, seal, reservation_name) in roots {
            let (root, lock) = match fence.prepared_roots.get(&disk.disk_id) {
                Some(prepared) => {
                    // The caller's FD only confirms its selection. All writes keep
                    // the resource and locks actually admitted before the fence.
                    reservation::verify_writer(&prepared.root, &prepared.writer)?;
                    (Arc::clone(&prepared.root), Arc::clone(&prepared.writer))
                }
                None => {
                    let directory_lock = root.directory(root.path())?;
                    reservation::lock(&directory_lock)?;
                    // A fence is never issued before this name is synchronized.
                    let lock = root.open_regular(&root.path().join(".writer.lock"))?;
                    reservation::lock(&lock)?;
                    let lock = crate::disk::FenceOwner::new(lock, Some(directory_lock));
                    reservation::verify_writer(&root, &lock)?;
                    (Arc::new(root), lock)
                }
            };
            verify_named_root(&root)?;
            inspect_new_root(&root, &seal, &reservation_name)?;
            resources.insert(
                disk.disk_id,
                MemberResource::Added(Box::new(PendingDisk {
                    root,
                    seal,
                    reservation_name,
                    _lock: lock,
                })),
            );
        }
        let mut members = Vec::new();
        for disk in &fence.intent.participants() {
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
        let mut data_disks = Vec::new();
        for disk in fence.intent.target.disks.iter().filter(|disk| {
            !disk.metadata_member
                && !fence
                    .intent
                    .origin
                    .config()
                    .disks
                    .iter()
                    .any(|old| old.disk_id == disk.disk_id)
        }) {
            let MemberResource::Added(resource) = resources
                .remove(&disk.disk_id)
                .expect("invariant: every added data root was admitted")
            else {
                unreachable!("invariant: additions own pending roots");
            };
            data_disks.push(data::PendingDataDisk {
                disk: disk.clone(),
                dependency: data::DataDependency::new(resource.seal.clone()),
                resource,
            });
        }
        assert!(
            resources.is_empty(),
            "invariant: every added root has an owner"
        );
        data_disks.sort_by(|a, b| a.disk.disk_id.cmp(&b.disk.disk_id));
        // Keep stable observable progress, independent of a caller's config order.
        members.sort_by(|a, b| a.disk.disk_id.cmp(&b.disk.disk_id));
        Ok(PendingMembership {
            source,
            transition_hash: fence.hash.clone(),
            archive_root: archive_root.to_owned(),
            members,
            target_metadata_members: fence.intent.target.metadata_members().len(),
            data_disks,
            ready_data_disks: Vec::new(),
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
                reservation::verify_writer(&handle.root, &handle._lock)?;
                inspect_new_root(&handle.root, &handle.seal, &handle.reservation_name).map(|_| ())
            }
        }
    }
}

pub(super) fn inspect_new_root(
    root: &DiskRoot,
    expected: &PendingDiskSeal,
    reservation_name: &str,
) -> Result<bool> {
    reservation::verify_marker(root, reservation_name)?;
    let names = root
        .entries(root.path())?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(fs_error)?;
    let stored = root.open_optional_read_regular(&root.path().join("disk.json"))?;
    let bound = match stored {
        None => false,
        Some(file) => {
            let actual: PendingDiskSeal = decode_strict(&read_file_bytes(file, MAX_METADATA)?)?;
            if &actual != expected {
                return Err(Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "pending disk seal differs from held member or transition",
                ));
            }
            true
        }
    };
    let expected_bytes = encode_metadata(expected, MAX_METADATA)?;
    for name in names {
        if platform_io::fs_safety::AtomicFile::is_temporary_name(&name) {
            let bytes = root.read_bytes(&root.path().join(name.as_path()), MAX_METADATA)?;
            // Only an exact prefix of this operation's immutable pending seal
            // explains a crash before rename. Preserve, never promote/delete it.
            if !expected_bytes.starts_with(&bytes) {
                return Err(Error::new(
                    ErrorCode::ObjectCorrupt,
                    "pending seal staging differs from its exact reservation",
                ));
            }
        } else {
            match name.as_os_str().to_str() {
                Some(value) if value == reservation_name => (),
                Some(".writer.lock") => {
                    if root
                        .open_regular(&root.path().join(".writer.lock"))?
                        .metadata()?
                        .len()
                        != 0
                    {
                        return Err(Error::new(
                            ErrorCode::DiskIdentityMismatch,
                            "pending lock contains unrelated bytes",
                        ));
                    }
                }
                Some("disk.json") if bound => (),
                Some("metadata") if bound => (),
                _ => {
                    return Err(Error::new(
                        ErrorCode::UnsafePath,
                        "pending root contains active or unrelated material",
                    ));
                }
            }
        }
    }
    if bound {
        validate_metadata_namespace(root, expected)?;
    }
    Ok(bound)
}
fn validate_metadata_namespace(root: &DiskRoot, expected: &PendingDiskSeal) -> Result<()> {
    let metadata = root.path().join("metadata");
    if root.optional_metadata(&metadata)?.is_none() {
        return Ok(());
    }
    for name in root.entries(&metadata)? {
        let name = name.map_err(fs_error)?;
        if !matches!(
            name.as_os_str().to_str(),
            Some("membership-pending" | "membership-decisions")
        ) {
            return Err(Error::new(
                ErrorCode::UnsafePath,
                "pending disk has an active metadata namespace",
            ));
        }
        let namespace = metadata.join(name.as_path());
        for entry in root.entries(&namespace)? {
            if entry.map_err(fs_error)?.as_os_str() != expected.transition_hash.as_str() {
                return Err(Error::new(
                    ErrorCode::ConfigConflict,
                    "pending disk has a different transition namespace",
                ));
            }
            if name.as_os_str() == "membership-decisions" {
                super::super::decision::DecisionStore::inspect_preparation(
                    root,
                    &expected.identity,
                    &expected.transition_hash,
                )?;
            }
        }
    }
    Ok(())
}
pub(in crate::engine::membership_transition) fn verify_named_root(
    root: &DiskRoot,
) -> Result<FileIdentity> {
    // Even a read-only name comparison is target I/O. Keep the original
    // bound responsibility before the first name lookup and through teardown;
    // None is only the explicit unbound pending/bootstrap provenance.
    let activity = root.retain_io_activity()?;
    let held = FileIdentity::from_fd(root.directory(root.path())?)?;
    if let Some(activity) = &activity {
        activity.check_running()?;
    }
    let named = DiskRoot::open(root.path())?;
    let observed = FileIdentity::from_fd(named.directory(named.path())?)?;
    if held != observed {
        return Err(Error::new(
            ErrorCode::DiskIdentityMismatch,
            "pending member root moved after admission",
        ));
    }
    if let Some(activity) = &activity {
        activity.check_running()?;
    }
    Ok(held)
}
pub(in crate::engine::membership_transition) fn member_identity(
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
/// Pure structural admission before the first durable fence. No target root is
/// inspected, and this proof does not establish directory readiness or an abort.
pub(in crate::engine::membership_transition) fn validate_pure_add_target(
    original: &ValidatedConfig,
    target: &ValidatedConfig,
    control_root: &Path,
) -> Result<Vec<DiskConfig>> {
    if !same_settings(original, target) {
        return Err(unsupported());
    }
    let old = original
        .disks
        .iter()
        .map(|disk| (disk.disk_id.as_str(), disk))
        .collect::<BTreeMap<_, _>>();
    let next = target
        .disks
        .iter()
        .map(|disk| (disk.disk_id.as_str(), disk))
        .collect::<BTreeMap<_, _>>();
    if old
        .iter()
        .any(|(id, disk)| next.get(id).is_none_or(|other| !same_disk(disk, other)))
    {
        return Err(unsupported());
    }
    let old_slots = original
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
            disk.binding_generation != 1 || old_slots.contains(&disk.slot_id.to_ascii_lowercase())
        })
    {
        return Err(unsupported());
    }
    if target
        .disks
        .iter()
        .any(|disk| disk.root.starts_with(control_root) || control_root.starts_with(&disk.root))
    {
        return Err(Error::new(
            ErrorCode::UnsafePath,
            "pending storage directory overlaps the ZXS control-state directory",
        ));
    }
    Ok(added)
}

fn unsupported() -> Error {
    Error::new(
        ErrorCode::ConfigConflict,
        "pending enrollment supports only unchanged original members plus fresh generation-one devices",
    )
}
pub(in crate::engine::membership_transition) fn same_disk(
    old: &DiskConfig,
    new: &DiskConfig,
) -> bool {
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
pub(in crate::engine::membership_transition) fn same_settings(old: &Config, new: &Config) -> bool {
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
