//! Read-only generation discovery/admission and explicit crash resume.
use super::*;

mod restricted;
pub(crate) use restricted::{discover_startup, discover_startup_in_store};
pub(super) use restricted::{verify_prepared_add, verify_prepared_exit_stop};

pub(crate) fn latest_restore_config(
    configs: &BTreeMap<String, (ValidatedConfig, usize)>,
) -> Result<ValidatedConfig> {
    let mut selected: Option<&(ValidatedConfig, usize)> = None;
    for candidate in configs
        .values()
        .filter(|(config, _)| config.generation().is_some())
    {
        match selected {
            None => selected = Some(candidate),
            Some(previous) => {
                let current = candidate
                    .0
                    .generation()
                    .expect("invariant: generation filtered");
                let old = previous
                    .0
                    .generation()
                    .expect("invariant: selected generation");
                if current.portable.contains(&old.hash)? {
                    selected = Some(candidate);
                } else if !old.portable.contains(&current.hash)? {
                    return Err(Error::new(
                        ErrorCode::LogFork,
                        "restore roots contain conflicting generation chains",
                    ));
                }
            }
        }
    }
    let (config, count) = selected.expect("invariant: caller found a generation anchor");
    let generation = config.generation().expect("invariant: selected generation");
    for (other, _) in configs.values() {
        if other.generation().is_none() {
            generation
                .original
                .verify_saved_metadata_membership(other)?;
        }
    }
    if *count < config.metadata_quorum() {
        return Err(Error::new(
            ErrorCode::MetadataQuorumUnavailable,
            "latest generation has no fixed metadata majority; older anchors cannot authorize rollback",
        ));
    }
    Ok(config.clone())
}

/// Read-only discovery. Config is a projection and cannot choose an authority.
/// A conflicting or damaged visible marker never falls back to the old epoch.
pub(crate) fn discover(config: ValidatedConfig) -> Result<ValidatedConfig> {
    // Existing Engine/pure-add callers remain explicitly unrestricted. Never
    // call this as fallback from bootstrap RestrictedProbes or a locator error.
    discover_unrestricted_legacy(config)
}

struct DiscoveryRoot {
    disk: DiskConfig,
    root: DiskRoot,
    authority: Option<Arc<PortableAuthority>>,
}

fn discover_unrestricted_legacy(config: ValidatedConfig) -> Result<ValidatedConfig> {
    let mut selected = None;
    let mut damaged_marker = None;
    let mut authenticated_markers = BTreeMap::new();
    let mut pending = config.io_disks().cloned().collect::<Vec<_>>();
    let mut visited = BTreeSet::new();
    while !pending.is_empty() {
        let mut held = Vec::with_capacity(pending.len());
        // Authenticate every already-authorized root before following any hint.
        // A predecessor fence on one root cannot race discovery of an Abort
        // marker on another root into probing the rejected target.
        for disk in std::mem::take(&mut pending) {
            if !visited.insert(disk.disk_id.clone()) || disk_unavailable(&disk)? {
                continue;
            }
            let root = DiskRoot::open(&disk.root)?;
            let authority = match marker_at(&root, &disk, &config) {
                Ok(authority) => authority.map(Arc::new),
                Err(error)
                    if matches!(
                        error.code(),
                        ErrorCode::InvalidJson | ErrorCode::ObjectCorrupt
                    ) =>
                {
                    // A damaged copy grants no locator or authority. Keep its
                    // configured vote in N/Q and require an intact fixed majority
                    // before returning any selected generation below.
                    damaged_marker.get_or_insert(error);
                    continue;
                }
                Err(error) => return Err(error),
            };
            if let Some(authority) = &authority {
                authenticated_markers.insert(disk.disk_id.clone(), authority.hash()?);
                select_authority(&mut selected, Arc::clone(authority))?;
            }
            held.push(DiscoveryRoot {
                disk,
                root,
                authority,
            });
        }
        for held in held {
            if let Some(authority) = &held.authority {
                // Abort retains the origin. Its rejected target is history,
                // never a root locator or permission to perform target I/O.
                append_unknown_disks(&mut pending, &config, &authority.effective_config()?.disks);
            }
            if config
                .disks
                .iter()
                .any(|old| old.disk_id == held.disk.disk_id && old.metadata_member)
            {
                append_fence_hints(&mut pending, &config, &held, selected.as_deref())?;
            }
        }
    }
    match selected {
        Some(portable) => {
            if damaged_marker.is_some() {
                let effective = portable.effective_config()?;
                let hash = portable.hash()?;
                let intact = effective
                    .metadata_members()
                    .filter(|disk| authenticated_markers.get(&disk.disk_id) == Some(&hash))
                    .count();
                if intact < effective.metadata_quorum() {
                    return Err(Error::new(
                        ErrorCode::MetadataQuorumUnavailable,
                        "damaged generation markers have no intact fixed-authority majority",
                    ));
                }
            }
            admit_discovered(Arc::unwrap_or_clone(portable), &config)
        }
        None => match damaged_marker {
            Some(error) => Err(error),
            None => Ok(config),
        },
    }
}

fn append_unknown_disks(
    pending: &mut Vec<DiskConfig>,
    config: &ValidatedConfig,
    disks: &[DiskConfig],
) {
    pending.extend(
        disks
            .iter()
            .filter(|candidate| {
                !config
                    .disks
                    .iter()
                    .any(|known| known.disk_id == candidate.disk_id)
            })
            .cloned(),
    );
}

fn append_fence_hints(
    pending: &mut Vec<DiskConfig>,
    config: &ValidatedConfig,
    held: &DiscoveryRoot,
    selected: Option<&PortableAuthority>,
) -> Result<()> {
    let DiscoveryRoot {
        disk,
        root,
        authority,
    } = held;
    // Hints do not authorize admission. Inspect only this held root's own
    // journal namespaces, with the complete authenticated batch lineage.
    let mut paths = vec![crate::journal::log_path(disk)];
    if let Ok(seal) =
        root.read_json_observed::<DiskSeal>(&disk.root.join("disk.json"), &mut |_| Ok(()))
        && let DiskSealFormat::MembershipGeneration { authority_hash } = seal.format
    {
        paths.push(generation_directory(disk, &authority_hash).join("journal.bin"));
    }
    for path in paths {
        if let Ok(scan) = crate::journal::scan_path_at(root, &path) {
            for (record, _) in scan
                .records
                .iter()
                .filter(|(record, _)| record.record_type == RecordType::MembershipFence)
            {
                let mut rejected = false;
                for authority in [selected, authority.as_deref()].into_iter().flatten() {
                    rejected |= authority.rejects_transition(&record.object_hash)?;
                }
                if rejected {
                    continue;
                }
                if let Ok(intent) =
                    load_object_at::<TransitionIntent>(root, disk, INTENT_KIND, &record.object_hash)
                    && config.same_metadata_membership(intent.origin.config())
                {
                    append_unknown_disks(pending, config, &intent.target.disks);
                }
            }
        }
    }
    Ok(())
}

fn marker_at(
    root: &DiskRoot,
    disk: &DiskConfig,
    projected: &ValidatedConfig,
) -> Result<Option<PortableAuthority>> {
    marker_path_at(root, &disk.root, projected)
}

fn marker_path_at(
    root: &DiskRoot,
    path: &Path,
    projected: &ValidatedConfig,
) -> Result<Option<PortableAuthority>> {
    let directory = path.join("metadata/pool-membership");
    let file = if root.optional_metadata(&path.join("metadata"))?.is_some()
        && root.optional_metadata(&directory)?.is_some()
    {
        root.directory(&directory)?;
        root.open_optional_read_regular(&path.join(GENERATION_FILE))?
    } else {
        None
    };
    let Some(file) = file else {
        if let Some(file) = root.open_optional_read_regular(&path.join("disk.json"))? {
            let bytes = read_file_bytes(file, MAX_METADATA)?;
            // This is the disk identity, not a generation marker. Preserve its
            // concrete decode error so discovery cannot isolate it as a damaged
            // marker or replace it with a generic genesis-recovery failure.
            let identity_error = |source| {
                Error::from(source).context(
                    ErrorCode::DiskIdentityMismatch,
                    "persisted disk identity cannot be decoded",
                )
            };
            let value: json_codec::Value = decode_strict(&bytes).map_err(identity_error)?;
            if value.get("format").and_then(json_codec::Value::as_str) == Some("membership-pending")
            {
                let _: PendingDiskSeal = decode_strict(&bytes).map_err(identity_error)?;
                return Ok(None);
            }
            let seal = decode_seal(&bytes)?;
            if matches!(seal.format, DiskSealFormat::MembershipGeneration { .. }) {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "active seal lacks its generation marker",
                ));
            }
        }
        return Ok(None);
    };
    let portable: PortableAuthority = decode_strict(&read_file_bytes(file, MAX_METADATA)?)?;
    if !portable.matches_metadata_projection(projected)? {
        return Err(Error::new(
            ErrorCode::ConfigConflict,
            "projection differs from authenticated generation lineage",
        ));
    }
    Ok(Some(portable))
}

fn select_authority(
    selected: &mut Option<Arc<PortableAuthority>>,
    authority: Arc<PortableAuthority>,
) -> Result<()> {
    if let Some(previous) = selected {
        let previous_hash = previous.hash()?;
        let hash = authority.hash()?;
        if previous_hash != hash && !authority.contains(&previous_hash)? {
            if !previous.contains(&hash)? {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "roots select conflicting generation authorities",
                ));
            }
            return Ok(());
        }
    }
    // Different valid witnesses for one core have the same generation hash.
    *selected = Some(authority);
    Ok(())
}

fn admit_discovered(
    portable: PortableAuthority,
    projected: &ValidatedConfig,
) -> Result<ValidatedConfig> {
    let intent = portable.intent()?;
    let hash = portable.hash()?;
    let transition = object_hash(INTENT_KIND, &intent)?;
    let original = portable.genesis_config()?;
    let epoch = portable.epoch()?;
    let mut target = portable.effective_config()?;
    let roots = target
        .disks
        .iter()
        .map(|disk| {
            projected
                .disks
                .iter()
                .find(|old| old.disk_id == disk.disk_id)
                .map_or_else(|| disk.root.clone(), |old| old.root.clone())
        })
        .collect();
    target = target
        .with_disk_roots(roots)?
        .with_workspace_limits(Some(projected.effective_workspace_limits()))?;
    let mut base = None;
    let mut failure = None;
    for disk in target.metadata_members() {
        if disk_unavailable(disk)? {
            continue;
        }
        let attempt = (|| {
            let root = DiskRoot::open(&disk.root)?;
            let path = member_directory(disk, &transition).join(CONTROL_ARCHIVE_DIRECTORY);
            portable.read_archive_base(root.private_child(&path)?)
        })();
        match attempt {
            Ok(value) => {
                base = Some(value);
                break;
            }
            Err(error) => {
                failure.get_or_insert(error);
            }
        }
    }
    let (state, history) = base.ok_or_else(|| {
        failure.unwrap_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "generation has no verified frozen bootstrap",
            )
        })
    })?;
    let generation = Arc::new(Generation {
        backup_continuation: None,
        hash,
        workspace_roots: portable.workspace_root_aliases()?,
        portable,
        intent: intent.clone(),
        original,
        history,
        epoch,
        base: Arc::new(state),
    });
    let portable = generation.portable.clone();
    portable.restrict_io(target.with_generation(generation)?)
}

/// A held root may still have its exact predecessor seal during interrupted
/// activation. This is not general pending-root admission.
pub(crate) fn restore_anchor(
    path: &Path,
    seal: &DiskSeal,
    original: ValidatedConfig,
) -> Result<ValidatedConfig> {
    let root = DiskRoot::open(path)?;
    let Some(file) = root.open_optional_read_regular(&path.join(GENERATION_FILE))? else {
        if matches!(seal.format, DiskSealFormat::MembershipGeneration { .. }) {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "active seal lacks generation authority",
            ));
        }
        return Ok(original);
    };
    let portable: PortableAuthority = decode_strict(&read_file_bytes(file, MAX_METADATA)?)?;
    original.verify_saved_metadata_membership(&portable.genesis_config()?)?;
    let target = portable.effective_config()?;
    let paths = target
        .disks
        .iter()
        .map(|disk| {
            if disk.disk_id == seal.disk_id {
                path.to_owned()
            } else {
                disk.root.clone()
            }
        })
        .collect();
    let target = target.with_disk_roots(paths)?;
    let intent = portable.intent()?;
    let transition = object_hash(INTENT_KIND, &intent)?;
    let disk = target
        .metadata_members()
        .find(|disk| disk.disk_id == seal.disk_id)
        .ok_or_else(|| {
            Error::new(
                ErrorCode::ConfigConflict,
                "explicit restore root is not an authenticated current metadata member",
            )
        })?;
    // Only this explicitly supplied retained root is an I/O authority here.
    // The saved configuration's other addresses are never recovery probes.
    // A portable graph and copied archive alone are not this root's vote. Admit
    // its actual physical enrollment and local decision before the caller can
    // count the anchor toward the fixed quorum or inspect any explicit data root.
    let hash = portable.hash()?;
    let parent = root.private_child(&member_directory(disk, &transition))?;
    if let Some(core) = portable.readmission_core() {
        core.verify_member(&root, disk, &hash, false)?;
        require_abort_journal(&root, disk, &hash)?;
    } else if let Some(core) = portable.terminal_resume_core() {
        core.verify_member(&root, disk)?;
        require_abort_journal(&root, disk, &hash)?;
    } else if portable.is_abort() {
        portable.verify_abort_member(&root, disk)?;
        require_abort_journal(&root, disk, &hash)?;
    } else {
        let joint = portable.add_joint()?;
        let verified = joint.verify()?;
        let identity = &verified.dependencies[&disk.disk_id].enrollment.identity;
        let physical = FileIdentity::from_fd(root.directory(root.path())?)?;
        if identity != &admission::member_identity(&target, disk, physical) {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "restore root differs from its authenticated enrollment",
            ));
        }
        let store = joint::store::JointStore::open(
            &root,
            &parent,
            identity,
            &transition,
            &joint.proposal_hash,
            false,
        )?;
        store.verify_certificate_read_only(&joint.certificate_hash, &joint.proposal_hash, disk)?;
    }
    if portable.exit_registry().is_some() {
        require_abort_journal(&root, disk, &hash)?;
    }
    let (state, history) = portable
        .read_archive_base(parent.private_child(&parent.path().join(CONTROL_ARCHIVE_DIRECTORY))?)?;
    portable.verify_registry_source(&state)?;
    let generation = Arc::new(Generation {
        backup_continuation: None,
        hash,
        workspace_roots: portable.workspace_root_aliases()?,
        original: portable.genesis_config()?,
        epoch: portable.epoch()?,
        portable,
        intent,
        history,
        base: Arc::new(state),
    });
    let portable = generation.portable.clone();
    portable.restrict_io(target.with_generation(generation)?)
}
impl ValidatedConfig {
    /// Called only for an explicitly supplied data root after the retained
    /// metadata quorum has selected this configuration and its current seal has
    /// been matched to the held directory. This grants no locator or vote.
    pub(crate) fn verify_explicit_recovery_data_root(
        &self,
        root: &DiskRoot,
        disk: &DiskConfig,
        seal: &DiskSeal,
    ) -> Result<()> {
        self.verify_root_membership(root, disk)?;
        if let Some(generation) = self.generation()
            && let Some(dependency) = generation.portable.data_dependency(&disk.disk_id)?
        {
            dependency.verify_active_seal(seal)?;
            dependency.verify_local(root)?;
        }
        Ok(())
    }

    pub(crate) fn verify_root_membership(&self, root: &DiskRoot, disk: &DiskConfig) -> Result<()> {
        let saved: ValidatedConfig = root.read_json_observed(
            &disk.root.join("metadata/pool-membership/config.json"),
            &mut |_| Ok(()),
        )?;
        match self.generation() {
            None => self.verify_saved_metadata_membership(&saved),
            Some(generation) => {
                generation
                    .original
                    .verify_saved_metadata_membership(&saved)?;
                let portable: PortableAuthority =
                    root.read_json_observed(&disk.root.join(GENERATION_FILE), &mut |_| Ok(()))?;
                if portable.hash()? != generation.hash {
                    return Err(Error::new(
                        ErrorCode::PublicationInvalid,
                        "root generation authority differs",
                    ));
                }
                portable.intent()?;
                Ok(())
            }
        }
    }
}

impl Engine {
    pub(in crate::engine) fn prepare_generation_resources(
        &mut self,
    ) -> Result<Vec<crate::engine::RecoverySealFailure>> {
        let Some(generation) = self.config.generation().cloned() else {
            return Ok(Vec::new());
        };
        if generation.portable.is_backup() {
            return self.prepare_backup_serving_resources();
        }
        generation
            .portable
            .verify_registry_source(&generation.base)?;
        let mut warnings = Vec::new();
        let mut rejected = std::collections::BTreeSet::new();
        for disk in self.config.metadata_members() {
            let Some(handle) = self.disk_resources.available(disk) else {
                continue;
            };
            let result = (|| {
                self.config.verify_root_membership(&handle.root, disk)?;
                let transition = object_hash(INTENT_KIND, &generation.intent)?;
                let parent = handle
                    .root
                    .private_child(&member_directory(disk, &transition))?;
                if let Some(core) = generation.portable.readmission_core() {
                    core.verify_member(&handle.root, disk, &generation.hash, true)?;
                    require_abort_journal(&handle.root, disk, &generation.hash)?;
                } else if let Some(core) = generation.portable.terminal_resume_core() {
                    core.verify_member(&handle.root, disk)?;
                    require_abort_journal(&handle.root, disk, &generation.hash)?;
                } else if generation.portable.is_abort() {
                    generation
                        .portable
                        .verify_abort_member(&handle.root, disk)?;
                    require_abort_journal(&handle.root, disk, &generation.hash)?;
                } else {
                    let joint = generation.portable.add_joint()?;
                    let verified = joint.verify()?;
                    let identity = &verified.dependencies[&disk.disk_id].enrollment.identity;
                    let physical =
                        FileIdentity::from_fd(handle.root.directory(handle.root.path())?)?;
                    if identity.device != physical.device || identity.inode != physical.inode {
                        return Err(Error::new(
                            ErrorCode::DiskIdentityMismatch,
                            "active member differs from joint identity",
                        ));
                    }
                    let store = joint::store::JointStore::open(
                        &handle.root,
                        &parent,
                        identity,
                        &transition,
                        &joint.proposal_hash,
                        false,
                    )?;
                    store.verify_certificate(
                        &joint.certificate_hash,
                        &joint.proposal_hash,
                        disk,
                    )?;
                }
                if generation.registry().is_some() {
                    require_abort_journal(&handle.root, disk, &generation.hash)?;
                }
                generation.portable.read_archive_base(
                    parent.private_child(&parent.path().join(CONTROL_ARCHIVE_DIRECTORY))?,
                )?;

                Ok(())
            })();
            if let Err(source) = result {
                rejected.insert(disk.disk_id.clone());
                warnings.push(crate::engine::RecoverySealFailure {
                    context: crate::engine::RecoverySealContext::MembershipAnchor {
                        disk_id: disk.disk_id.clone(),
                        slot_id: disk.slot_id.clone(),
                    },
                    source,
                });
            }
        }
        for disk in self
            .config
            .disks
            .iter()
            .filter(|disk| !disk.metadata_member)
        {
            if let Some(core) = generation.portable.readmission_core()
                && core.target_predecessor_seal(&disk.disk_id)?.is_some()
                && let Some(handle) = self.disk_resources.available(disk)
            {
                core.verify_member(&handle.root, disk, &generation.hash, true)?;
            }

            let Some(handle) = self.disk_resources.available(disk) else {
                continue;
            };
            let Some(dependency) = generation.portable.data_dependency(&disk.disk_id)? else {
                continue;
            };
            let result = (|| {
                self.config.verify_root_membership(&handle.root, disk)?;
                dependency.verify_active_seal(&handle.seal)?;
                dependency.verify_local(&handle.root)
            })();
            if let Err(source) = result {
                rejected.insert(disk.disk_id.clone());
                warnings.push(crate::engine::RecoverySealFailure {
                    context: crate::engine::RecoverySealContext::MembershipAnchor {
                        disk_id: disk.disk_id.clone(),
                        slot_id: disk.slot_id.clone(),
                    },
                    source,
                });
            }
        }
        if self
            .config
            .metadata_members()
            .filter(|disk| {
                self.disk_resources.available(disk).is_some() && !rejected.contains(&disk.disk_id)
            })
            .count()
            < self.config.metadata_quorum()
        {
            let message = "generation bootstrap lacks its fixed metadata quorum";
            return Err(match warnings.pop() {
                Some(failure) => failure
                    .source
                    .context(ErrorCode::MetadataQuorumUnavailable, message),
                None => Error::new(ErrorCode::MetadataQuorumUnavailable, message),
            });
        }
        if !rejected.is_empty() {
            self.disk_resources = Arc::new(self.disk_resources.excluding(&rejected));
        }
        self.state = Arc::clone(&generation.base);
        self.snapshot = generation.snapshot();
        self.next_slot = generation.first_sequence()?;
        Ok(warnings)
    }
}

fn require_abort_journal(root: &DiskRoot, disk: &DiskConfig, hash: &str) -> Result<()> {
    // Abort and Exit install this durable empty leaf before their active marker.
    // A healthy frozen archive cannot recreate a lost current-generation log.
    root.open_read_regular(&generation_directory(disk, hash).join("journal.bin"))
        .map(|_| ())
        .map_err(|source| {
            source.context(
                ErrorCode::RecoveryRequired,
                format!(
                    "control generation journal is unavailable for {}",
                    disk.disk_id
                ),
            )
        })
}

/// Missing *parents* are expected only before this root's marker/seal promotion.
/// A published/current marker or seal cannot use that absence to erase a fence.
fn activation_namespace_present(
    root: &DiskRoot,
    disk: &DiskConfig,
    config: &ValidatedConfig,
) -> Result<bool> {
    let generation = config
        .generation()
        .expect("invariant: activation has a generation");
    let parent = disk.root.join("metadata/membership-generations");
    if root.optional_metadata(&parent)?.is_some()
        && root
            .optional_metadata(&generation_directory(disk, &generation.hash))?
            .is_some()
        && (!(generation.portable.is_abort() || generation.registry().is_some())
            || root
                .open_optional_read_regular(
                    &generation_directory(disk, &generation.hash).join("journal.bin"),
                )?
                .is_some())
    {
        return Ok(true);
    }
    // An unmarked staging directory may precede the first durable leaf.
    // A current marker or active seal below makes absence corruption, not
    // permission to reconstruct an empty journal from the frozen base.
    let bytes = root.read_bytes(&disk.root.join("disk.json"), MAX_METADATA)?;
    let expected = identity_at(config, disk, &root.directory(root.path())?)?;
    predecessor_seal_matches(config, disk, root, &bytes, &expected)?;
    let value: json_codec::Value = decode_strict(&bytes)?;
    if value.get("format").and_then(json_codec::Value::as_str) != Some("membership-pending") {
        let seal = decode_seal(&bytes)?;
        if seal.format == expected.format {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                format!(
                    "active generation namespace or journal is missing for {}",
                    disk.disk_id
                ),
            ));
        }
    }
    let membership = disk.root.join("metadata/pool-membership");
    if root.optional_metadata(&membership)?.is_some()
        && let Some(file) = root.open_optional_read_regular(&disk.root.join(GENERATION_FILE))?
    {
        let portable: PortableAuthority = decode_strict(&read_file_bytes(file, MAX_METADATA)?)?;
        portable.intent()?;
        let hash = portable.hash()?;
        if Some(hash.as_str()) != generation.intent.origin.predecessor() {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                format!(
                    "prepared generation marker lacks its namespace or journal for {}",
                    disk.disk_id
                ),
            ));
        }
    }
    // The admitted seal is the exact predecessor (or this operation's pending
    // seal), and no current marker exists here. Explicit promotion will create
    // this new namespace; do not create it just to perform the read-only check.
    Ok(false)
}

impl Engine {
    /// Explicit retry after a crash during prepared markers or seal promotion.
    /// Routine open remains read-only at this boundary and never adopts pending roots.
    pub fn resume_membership_activation(root: &Path) -> Result<Self> {
        let (root, projected) = Self::recovery_projection(root, None)?;
        let stop = super::exit::stop_plan::prepare_resume(&root, &projected)?;
        let extension = if stop.is_none() {
            super::root_extension::prepare_resume(&root, &projected)?
        } else {
            None
        };
        let (config, bootstrap) = match (&stop, &extension) {
            (Some(stop), _) => (stop.config(), None),
            (_, Some(extension)) => (extension.config(), None),
            (None, None) => {
                let (config, bootstrap) = discover_startup(&root, projected)?;
                (config, Some(bootstrap))
            }
        };
        if config.generation().is_none() {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "no prepared generation authority",
            ));
        }
        let resources = Arc::new(DiskResources::open_activation(&config)?);
        // A completed predecessor marker is not an activation request for the
        // next fenced operation. Reject before promoting or repairing any root.
        for disk in config.metadata_members() {
            let held = &resources.get(disk)?.root;
            if !activation_namespace_present(held, disk, &config)? {
                continue;
            }
            let scan =
                crate::journal::scan_configured_at(held, disk, &config).map_err(|error| {
                    let code = error.code();
                    error.context(
                        code,
                        format!("inspect activation journal for {}", disk.disk_id),
                    )
                })?;
            if scan
                .records
                .iter()
                .any(|(record, _)| record.record_type == RecordType::MembershipFence)
            {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "current generation remains fenced; no successor activation marker",
                ));
            }
        }
        let mut engine = Self::prepare_recovery_resources(root, config, resources)?;
        if let Some(stop) = stop {
            stop.publish(&engine)?;
        }
        if let Some(extension) = extension {
            extension.publish(&engine)?;
        }
        drop(bootstrap);
        engine.admit_disk_exit_io_exclusion()?;
        engine.promote_generation_resources()?;
        let mut prepared = engine.prepare_recovery()?;
        engine.preflight_workspace_receipts(&mut prepared)?;
        engine.recover_prepared(crate::engine::RecoveryMode::CompletePending, prepared)?;
        engine.complete_generation_activation()?;
        engine.restore_current_disk_exit()?;
        engine.load_workspaces()?;
        engine.rebuild_index()?;
        Ok(engine)
    }
}
