//! Read-only generation discovery/admission and explicit crash resume.
use super::*;

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
    let mut selected: Option<PortableAuthority> = None;
    let mut pending = config.disks.clone();
    let mut visited = std::collections::BTreeSet::new();
    let mut invalid = None;
    while let Some(disk) = pending.pop() {
        if !visited.insert(disk.disk_id.clone()) || disk_unavailable(&disk)? {
            continue;
        }
        let root = DiskRoot::open(&disk.root)?;
        // Routing hints never authorize admission. Read each held root's own
        // generation namespace; stale system projections must find a later fence.
        if config
            .disks
            .iter()
            .any(|old| old.disk_id == disk.disk_id && old.metadata_member)
        {
            let mut paths = vec![crate::journal::log_path(&disk)];
            if let Ok(seal) =
                root.read_json_observed::<DiskSeal>(&disk.root.join("disk.json"), &mut |_| Ok(()))
                && let DiskSealFormat::MembershipGeneration { authority_hash } = seal.format
            {
                paths.push(generation_directory(&disk, &authority_hash).join("journal.bin"));
            }
            for path in paths {
                if let Ok(scan) = crate::journal::scan_path_at(&root, &path) {
                    for (record, _) in scan
                        .records
                        .iter()
                        .filter(|(record, _)| record.record_type == RecordType::MembershipFence)
                    {
                        if let Ok(intent) = load_object_at::<TransitionIntent>(
                            &root,
                            &disk,
                            INTENT_KIND,
                            &record.object_hash,
                        ) && config.same_metadata_membership(intent.origin.config())
                        {
                            pending.extend(
                                intent
                                    .target
                                    .disks
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
                    }
                }
            }
        }
        let candidate = (|| {
            let directory = disk.root.join("metadata/pool-membership");
            let file = if root
                .optional_metadata(&disk.root.join("metadata"))?
                .is_some()
                && root.optional_metadata(&directory)?.is_some()
            {
                root.directory(&directory)?;
                root.open_optional_regular(&disk.root.join(GENERATION_FILE))?
            } else {
                None
            };
            let Some(file) = file else {
                // A fence may name an existing empty target before enrollment.
                // Absence here is not authority and does not admit that root.
                let Some(seal_file) = root.open_optional_regular(&disk.root.join("disk.json"))?
                else {
                    return Ok(None);
                };
                let seal_bytes = read_file_bytes(seal_file, MAX_METADATA)?;
                if let Ok(seal) = decode_seal(&seal_bytes)
                    && matches!(seal.format, DiskSealFormat::MembershipGeneration { .. })
                {
                    return Err(Error::new(
                        ErrorCode::RecoveryRequired,
                        "active seal has no generation marker",
                    ));
                }
                return Ok(None);
            };
            let authority: PortableAuthority =
                decode_strict(&read_file_bytes(file, MAX_METADATA)?)?;
            let intent = authority.intent()?;
            let lineage = authority.lineage()?;
            if !lineage.iter().any(|(_, epoch)| {
                config.same_metadata_membership(epoch.origin.config())
                    || config.same_metadata_membership(&epoch.target)
            }) {
                return Err(Error::new(
                    ErrorCode::ConfigConflict,
                    "system projection differs from generation lineage",
                ));
            }
            pending.extend(
                intent
                    .target
                    .disks
                    .iter()
                    .filter(|candidate| {
                        !config
                            .disks
                            .iter()
                            .any(|known| known.disk_id == candidate.disk_id)
                    })
                    .cloned(),
            );
            Ok(Some(authority))
        })();
        match candidate {
            Ok(Some(authority)) => {
                if let Some(previous) = &selected {
                    let previous_hash = previous.hash()?;
                    let hash = authority.hash()?;
                    if previous_hash == hash || authority.contains(&previous_hash)? {
                        selected = Some(authority);
                    } else if !previous.contains(&hash)? {
                        return Err(Error::new(
                            ErrorCode::LogFork,
                            "roots select conflicting generation authorities",
                        ));
                    }
                } else {
                    selected = Some(authority);
                }
            }
            Ok(None) => (),
            Err(error) => {
                invalid.get_or_insert(error);
            }
        }
    }
    match selected {
        Some(portable) => admit_discovered(portable, &config),
        None => match invalid {
            Some(error) => Err(error),
            None => Ok(config),
        },
    }
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
    let mut target = intent.target.clone();
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
            read_generation_base(
                root.private_child(&path)?,
                &portable.archive_hash()?,
                &transition,
                &intent,
            )
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
        hash,
        portable,
        intent: intent.clone(),
        original,
        history,
        epoch,
        base: Arc::new(state),
    });
    target.with_generation(generation)
}

/// A held root may still have its exact predecessor seal during interrupted
/// activation. This is not general pending-root admission.
pub(crate) fn restore_anchor(
    path: &Path,
    seal: &DiskSeal,
    original: ValidatedConfig,
) -> Result<ValidatedConfig> {
    let root = DiskRoot::open(path)?;
    let Some(file) = root.open_optional_regular(&path.join(GENERATION_FILE))? else {
        if matches!(seal.format, DiskSealFormat::MembershipGeneration { .. }) {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "active seal lacks generation authority",
            ));
        }
        return Ok(original);
    };
    let portable: PortableAuthority = decode_strict(&read_file_bytes(file, MAX_METADATA)?)?;
    let intent = portable.intent()?;
    original.verify_saved_metadata_membership(&portable.genesis_config()?)?;
    let target = intent.target.clone();
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
    admit_discovered(portable, &target.with_disk_roots(paths)?)
}
impl ValidatedConfig {
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
                let verified = generation.portable.joint.verify()?;
                let identity = &verified.dependencies[&disk.disk_id].enrollment.identity;
                let physical = FileIdentity::from_fd(handle.root.directory(handle.root.path())?)?;
                if identity.device != physical.device || identity.inode != physical.inode {
                    return Err(Error::new(
                        ErrorCode::DiskIdentityMismatch,
                        "active member differs from joint identity",
                    ));
                }
                let store = joint::store::JointStore::open(
                    &parent,
                    identity,
                    &generation.portable.joint.proposal_hash,
                    false,
                )?;
                store.verify_certificate(
                    &generation.portable.joint.certificate_hash,
                    &generation.portable.joint.proposal_hash,
                    disk,
                )?;
                read_generation_base(
                    parent.private_child(&parent.path().join(CONTROL_ARCHIVE_DIRECTORY))?,
                    &generation.portable.archive_hash()?,
                    &transition,
                    &generation.intent,
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

impl Engine {
    /// Explicit retry after a crash during prepared markers or seal promotion.
    /// Routine open remains read-only at this boundary and never adopts pending roots.
    pub fn resume_membership_activation(root: &Path) -> Result<Self> {
        let root = existing_private_dir(root)?.path().to_path_buf();
        let config = discover(ValidatedConfig::try_from(read_json::<Config>(
            &root.join("config.json"),
        )?)?)?;
        if config.generation().is_none() {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "no prepared generation authority",
            ));
        }
        let resources = Arc::new(DiskResources::open_activation(&config)?);
        let mut engine = Self::prepare_recovery_resources(root, config, resources)?;
        engine.promote_generation_resources()?;
        let mut prepared = engine.prepare_recovery()?;
        engine.preflight_workspace_receipts(&mut prepared)?;
        engine.recover_prepared(crate::engine::RecoveryMode::CompletePending, prepared)?;
        engine.complete_generation_activation()?;
        engine.load_workspaces()?;
        engine.rebuild_index()?;
        Ok(engine)
    }
}
