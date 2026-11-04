//! Fixed-probe authority reconstruction before Engine resource admission.
//! Publication replay remains a separate gate; probes never choose a new N/Q.
use super::*;
use crate::engine::disk_lifecycle::bootstrap::{
    BootstrapError, LoadedLocator, ReadProbe, RestrictedProbes, StopHint,
    runtime::{management_error, open_store},
    store::BootstrapStore,
};

#[derive(Debug, error_definition::Error)]
pub(crate) enum RestrictedDiscoveryFailure {
    #[error(transparent)]
    Bootstrap(#[from] BootstrapError),
    #[error(transparent)]
    Verification(#[from] Error),
}

/// Independently verified authority and local evidence. The private Config is
/// handed to startup only after stop reconciliation. The fixed N/Q comes from
/// authenticated member records, never from the locator or number of probes.
#[derive(Debug)]
pub(crate) struct RestrictedGenerationEvidence {
    config: ValidatedConfig,
    #[cfg(test)]
    authority_hash: String,
    metadata_members: usize,
    metadata_quorum: usize,
    verified_members: Vec<String>,
    rejected_members: Vec<(String, Error)>,
}
#[cfg(test)]
impl RestrictedGenerationEvidence {
    pub(crate) fn authority_hash(&self) -> &str {
        &self.authority_hash
    }
    pub(crate) fn metadata_members(&self) -> usize {
        self.metadata_members
    }
    pub(crate) fn metadata_quorum(&self) -> usize {
        self.metadata_quorum
    }
    pub(crate) fn verified_members(&self) -> &[String] {
        &self.verified_members
    }
    pub(crate) fn rejected_members(&self) -> &[(String, Error)] {
        &self.rejected_members
    }
}

pub(crate) fn discover_startup(
    root: &Path,
    projected: ValidatedConfig,
) -> Result<(ValidatedConfig, BootstrapStore)> {
    discover_startup_with_io(root, projected, &mut NamedRootIo)
}

fn discover_startup_with_io(
    root: &Path,
    projected: ValidatedConfig,
    io: &mut impl RootIo,
) -> Result<(ValidatedConfig, BootstrapStore)> {
    let terminal = super::super::terminal_resume::completed_selector(root)?;
    let store = open_store(root)?;
    let config = discover_startup_in_store_with_io(&store, projected, io)?;
    if let Some(terminal) = terminal {
        terminal.verify_selected(&config)?;
    }
    Ok((config, store))
}

pub(crate) fn discover_startup_in_store(
    store: &BootstrapStore,
    projected: ValidatedConfig,
) -> Result<ValidatedConfig> {
    discover_startup_in_store_with_io(store, projected, &mut NamedRootIo)
}

fn discover_startup_in_store_with_io(
    store: &BootstrapStore,
    projected: ValidatedConfig,
    io: &mut impl RootIo,
) -> Result<ValidatedConfig> {
    let locator = store.reload(&projected.pool_id).map_err(management_error)?;
    let evidence =
        discover_locator_with_io(&locator, &projected, io).map_err(|error| match error {
            RestrictedDiscoveryFailure::Bootstrap(error) => management_error(error),
            RestrictedDiscoveryFailure::Verification(error) => error,
        })?;
    for stop in locator.stop_hints() {
        let generation = evidence.config.generation().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "pending bootstrap stop requires explicit retained-root management recovery",
            )
        })?;
        generation.portable.verify_bootstrap_stop(
            &crate::DiskRegistrationIdentity {
                pool_id: stop.pool_id.into(),
                disk_id: stop.disk_id.into(),
                slot_id: stop.slot_id.into(),
                binding_generation: stop.binding_generation,
            },
            stop.operation_id,
            &stop.intent_digest.to_string(),
        )?;
    }
    let forbidden = evidence
        .config
        .generation()
        .map(|generation| generation.portable.forbidden_disk_ids())
        .transpose()?
        .unwrap_or_default();
    let config = locator.restrict_config(evidence.config, &projected)?;
    let allowed = config
        .io_disks()
        .filter(|disk| !forbidden.contains(&disk.disk_id))
        .map(|disk| disk.disk_id.clone())
        .collect();
    let config = config
        .restrict_io_disks(allowed)?
        .with_workspace_limits(Some(projected.effective_workspace_limits()))?;
    Ok(config)
}

// The only named storage-root I/O boundary. In contrast to disk_unavailable,
// stat does not enumerate contents before the held directory identity matches.
// Tests replace this boundary, not the selection/verification implementation.
trait RootIo {
    fn stat(&mut self, probe: ReadProbe<'_>) -> std::io::Result<std::fs::Metadata>;
    fn open(&mut self, probe: ReadProbe<'_>) -> Result<DiskRoot>;
}
struct NamedRootIo;
impl RootIo for NamedRootIo {
    fn stat(&mut self, probe: ReadProbe<'_>) -> std::io::Result<std::fs::Metadata> {
        std::fs::symlink_metadata(probe.path())
    }
    fn open(&mut self, probe: ReadProbe<'_>) -> Result<DiskRoot> {
        DiskRoot::open(probe.path())
    }
}
struct ProbeCandidate<'a> {
    probe: ReadProbe<'a>,
    disk: Option<DiskConfig>,
}
struct HeldCandidate {
    disk: DiskConfig,
    root: DiskRoot,
    authority_hash: Option<String>,
}

fn candidates<'a>(
    probes: RestrictedProbes<'a>,
    config: &ValidatedConfig,
) -> Result<Vec<ProbeCandidate<'a>>> {
    let mut candidates = Vec::with_capacity(probes.len());
    probes.try_for_each(|probe| {
        let binding = probe.binding();
        let disk = config
            .disks
            .iter()
            .find(|disk| disk.disk_id == probe.disk_id());
        if binding.pool_id != config.pool_id
            || disk.is_some_and(|disk| {
                binding.slot_id != disk.slot_id
                    || binding.binding_generation != disk.binding_generation
                    || disk.root.as_os_str() != probe.path().as_os_str()
            })
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "restricted probe cannot be rebound to another projection",
            ));
        }
        candidates.push(ProbeCandidate {
            probe,
            disk: disk.cloned(),
        });
        Ok(())
    })?;
    Ok(candidates)
}

#[cfg(test)]
fn discover_store_with_io(
    store: &BootstrapStore,
    config: &ValidatedConfig,
    io: &mut impl RootIo,
) -> std::result::Result<RestrictedGenerationEvidence, RestrictedDiscoveryFailure> {
    let locator = store.reload(&config.pool_id)?;
    discover_locator_with_io(&locator, config, io)
}

fn discover_locator_with_io(
    locator: &LoadedLocator,
    config: &ValidatedConfig,
    io: &mut impl RootIo,
) -> std::result::Result<RestrictedGenerationEvidence, RestrictedDiscoveryFailure> {
    let hints: Vec<_> = locator.stop_hints().collect();
    Ok(discover_probes_with_io(
        locator.select(config)?,
        config,
        &hints,
        io,
    )?)
}

#[cfg(test)]
fn discover_with_io(
    probes: RestrictedProbes<'_>,
    config: &ValidatedConfig,
    io: &mut impl RootIo,
) -> Result<RestrictedGenerationEvidence> {
    discover_probes_with_io(probes, config, &[], io)
}

fn discover_probes_with_io(
    probes: RestrictedProbes<'_>,
    config: &ValidatedConfig,
    stops: &[StopHint<'_>],
    io: &mut impl RootIo,
) -> Result<RestrictedGenerationEvidence> {
    // Construct the complete immutable candidate list before any root I/O.
    // No fence, authority, archive or Config path can append to this list.
    let candidates = candidates(probes, config)?;
    let mut held = Vec::with_capacity(candidates.len());
    let mut selected = None;
    let mut damaged = Vec::new();
    for ProbeCandidate { probe, disk } in candidates {
        match io.stat(probe) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
            Ok(_) => (),
        }
        let root = io.open(probe)?;
        if FileIdentity::from_fd(root.directory(root.path())?)? != probe.expected_identity() {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "retained discovery root differs from the locator's held observation",
            ));
        }
        // Identity is checked before the first seal/marker/archive read.
        let marker = match discover_root_authority(&root, probe.path(), config, stops) {
            Ok(marker) => marker,
            Err(error)
                if matches!(
                    error.code(),
                    ErrorCode::InvalidJson | ErrorCode::ObjectCorrupt
                ) =>
            {
                damaged.push((probe.disk_id().to_owned(), error));
                continue;
            }
            Err(error) => return Err(error),
        };
        let disk = match disk {
            Some(disk) => disk,
            None => {
                let Some(authority) = &marker else {
                    continue;
                };
                let effective = authority.effective_config()?;
                let binding = probe.binding();
                let mut disk = effective
                    .disks
                    .iter()
                    .find(|disk| {
                        disk.disk_id == binding.disk_id
                            && disk.slot_id == binding.slot_id
                            && disk.binding_generation == binding.binding_generation
                    })
                    .cloned()
                    .ok_or_else(|| {
                        Error::new(
                            ErrorCode::RecoveryRequired,
                            "recorded new root is outside authenticated successor",
                        )
                    })?;
                disk.root = probe.path().to_path_buf();
                disk
            }
        };
        let authority_hash = marker.as_ref().map(PortableAuthority::hash).transpose()?;
        if let Some(authority) = marker {
            select_authority(&mut selected, Arc::new(authority))?;
        }
        held.push(HeldCandidate {
            disk,
            root,
            authority_hash,
        });
    }
    match selected {
        Some(portable) => {
            let mut evidence = verify_held_generation(Arc::unwrap_or_clone(portable), held)
                .map_err(|error| match damaged.pop() {
                    Some((disk, source)) => source.context(
                        error.code(),
                        format!("{}; damaged marker on {disk}", error.message()),
                    ),
                    None => error,
                })?;
            evidence.rejected_members.extend(damaged);
            Ok(evidence)
        }
        None if !damaged.is_empty() => {
            let (disk, source) = damaged.pop().expect("invariant: damaged marker present");
            Err(source.context(
                ErrorCode::RecoveryRequired,
                format!("damaged generation marker on {disk} cannot authorize genesis fallback"),
            ))
        }
        None => verify_held_genesis(config, held),
    }
}

fn discover_root_authority(
    root: &DiskRoot,
    path: &Path,
    projected: &ValidatedConfig,
    stops: &[StopHint<'_>],
) -> Result<Option<PortableAuthority>> {
    let mut selected = marker_path_at(root, path, projected)?.map(Arc::new);
    for stop in stops {
        let directory = path.join("metadata/membership-exits");
        if root.optional_metadata(&directory)?.is_none() {
            continue;
        }
        let directory = directory.join(stop.intent_digest.to_string());
        if root.optional_metadata(&directory)?.is_none() {
            continue;
        }
        let Some(file) = root.open_optional_read_regular(&directory.join("authority.json"))? else {
            continue;
        };
        let portable: PortableAuthority = decode_strict(&read_file_bytes(file, MAX_METADATA)?)?;
        if !portable.matches_metadata_projection(projected)? {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "prepared exit authority differs from projected lineage",
            ));
        }
        portable.verify_bootstrap_stop(
            &crate::DiskRegistrationIdentity {
                pool_id: stop.pool_id.into(),
                disk_id: stop.disk_id.into(),
                slot_id: stop.slot_id.into(),
                binding_generation: stop.binding_generation,
            },
            stop.operation_id,
            &stop.intent_digest.to_string(),
        )?;
        select_authority(&mut selected, Arc::new(portable))?;
    }
    Ok(selected.map(Arc::unwrap_or_clone))
}

fn verify_held_generation(
    portable: PortableAuthority,
    held: Vec<HeldCandidate>,
) -> Result<RestrictedGenerationEvidence> {
    verify_held_generation_from(portable, held, GenerationEvidenceSource::Markers)
}

#[derive(Clone, Copy)]
enum GenerationEvidenceSource {
    Markers,
    PreparedRootExtension,
}

fn verify_held_generation_from(
    portable: PortableAuthority,
    held: Vec<HeldCandidate>,
    source: GenerationEvidenceSource,
) -> Result<RestrictedGenerationEvidence> {
    let intent = portable.intent()?;
    let hash = portable.hash()?;
    let transition = object_hash(INTENT_KIND, &intent)?;
    let effective = portable.effective_config()?;
    let joint = if portable.is_abort() || portable.terminal_resume_core().is_some() {
        None
    } else {
        Some(portable.add_joint()?.verify()?)
    };
    let mut config = None;
    let mut evidence = RestrictedGenerationEvidence {
        config: effective.clone(),
        #[cfg(test)]
        authority_hash: hash.clone(),
        metadata_members: effective.metadata_members().len(),
        metadata_quorum: effective.metadata_quorum(),
        verified_members: Vec::new(),
        rejected_members: Vec::new(),
    };
    for held in held {
        let Some(member) = effective
            .metadata_members()
            .find(|disk| disk.disk_id == held.disk.disk_id)
        else {
            continue;
        };
        let result = (|| {
            if member.slot_id != held.disk.slot_id
                || member.binding_generation != held.disk.binding_generation
                || (matches!(source, GenerationEvidenceSource::Markers)
                    && held.authority_hash.as_deref() != Some(hash.as_str()))
            {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "retained member has no exact selected generation",
                ));
            }
            // The authenticated member supplies identity/role, never its address.
            let mut disk = member.clone();
            disk.root = held.disk.root.clone();
            let saved_path = disk.root.join("metadata/pool-membership/config.json");
            let saved = match source {
                GenerationEvidenceSource::Markers => {
                    Some(held.root.open_read_regular(&saved_path)?)
                }
                GenerationEvidenceSource::PreparedRootExtension => {
                    held.root.open_optional_read_regular(&saved_path)?
                }
            };
            if let Some(saved) = saved {
                let saved: ValidatedConfig = decode_strict(&read_file_bytes(saved, MAX_METADATA)?)?;
                portable
                    .genesis_config()?
                    .verify_saved_metadata_membership(&saved)?;
            } else if intent
                .origin
                .config()
                .disks
                .iter()
                .any(|old| old.disk_id == disk.disk_id)
            {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "retained Add member lost its saved genesis membership",
                ));
            }
            // Before promotion a newly enrolled root has only its pending seal,
            // certificate and complete archive. Those are verified below; a
            // missing derived config must not count as a verified vote itself.
            let parent = held
                .root
                .private_child(&member_directory(&disk, &transition))?;
            match &joint {
                None => {
                    if let Some(core) = portable.terminal_resume_core() {
                        core.verify_member(&held.root, &disk)?;
                    } else {
                        portable.verify_abort_member(&held.root, &disk)?;
                    }
                    require_abort_journal(&held.root, &disk, &hash)?;
                }
                Some(verified) => {
                    let dependency = verified.dependencies.get(&disk.disk_id).ok_or_else(|| {
                        Error::new(
                            ErrorCode::RecoveryRequired,
                            "selected authority lacks member dependency",
                        )
                    })?;
                    let physical = FileIdentity::from_fd(held.root.directory(held.root.path())?)?;
                    let identity = &dependency.enrollment.identity;
                    if identity != &admission::member_identity(&effective, &disk, physical) {
                        return Err(Error::new(
                            ErrorCode::DiskIdentityMismatch,
                            "retained root differs from joint enrollment",
                        ));
                    }
                    let joint = portable.add_joint()?;
                    let store = joint::store::JointStore::open(
                        &held.root,
                        &parent,
                        identity,
                        &transition,
                        &joint.proposal_hash,
                        false,
                    )?;
                    store.verify_certificate_read_only(
                        &joint.certificate_hash,
                        &joint.proposal_hash,
                        &disk,
                    )?;
                }
            }
            let (state, history) = portable.read_archive_base(
                parent.private_child(&parent.path().join(CONTROL_ARCHIVE_DIRECTORY))?,
            )?;
            portable.verify_registry_source(&state)?;
            if config.is_none() {
                config = Some(effective.clone().with_generation(Arc::new(Generation {
                    hash: hash.clone(),
                    workspace_roots: portable.workspace_root_aliases()?,
                    portable: portable.clone(),
                    intent: intent.clone(),
                    original: portable.genesis_config()?,
                    history,
                    epoch: portable.epoch()?,
                    base: Arc::new(state),
                }))?);
            }
            let config = config
                .as_ref()
                .expect("invariant: independently verified archive installed");
            let expected = identity_at(config, &disk, &held.root.directory(held.root.path())?)?;
            let seal = held
                .root
                .read_bytes(&disk.root.join("disk.json"), MAX_METADATA)?;
            predecessor_seal_matches(config, &disk, &held.root, &seal, &expected)?;
            if portable.exit_registry().is_some() {
                // A prepared envelope may precede active marker promotion.
                // Every registry-bearing successor, including Add and Abort,
                // must retain its own journal after marker/seal promotion.
                activation_namespace_present(&held.root, &disk, config)?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => evidence.verified_members.push(held.disk.disk_id),
            Err(error) => evidence.rejected_members.push((held.disk.disk_id, error)),
        }
    }
    if evidence.verified_members.len() < evidence.metadata_quorum {
        let message = format!(
            "restricted discovery verified {} of fixed {} metadata members; quorum is {}",
            evidence.verified_members.len(),
            evidence.metadata_members,
            evidence.metadata_quorum,
        );
        return Err(match evidence.rejected_members.pop() {
            Some((_, source)) => source.context(ErrorCode::MetadataQuorumUnavailable, message),
            None => Error::new(ErrorCode::MetadataQuorumUnavailable, message),
        });
    }
    evidence.config = config.expect("invariant: fixed majority verified a generation archive");
    Ok(evidence)
}

/// Only the exact SYSTEM Add plan may select this authority before its first
/// marker. Every vote still requires this root's own certificate, physical
/// enrollment and complete archive. The locator supplies no vote or authority.
pub(in crate::engine::membership_transition::pending::activation) fn verify_prepared_add(
    locator: &LoadedLocator,
    portable: PortableAuthority,
    projected: &ValidatedConfig,
) -> Result<ValidatedConfig> {
    verify_prepared_add_with_io(locator, portable, projected, &mut NamedRootIo)
}

fn verify_prepared_add_with_io(
    locator: &LoadedLocator,
    portable: PortableAuthority,
    projected: &ValidatedConfig,
    io: &mut impl RootIo,
) -> Result<ValidatedConfig> {
    portable.add_joint()?.verify()?;
    let intent = portable.intent()?;
    if portable.is_abort()
        || portable.is_exit()
        || !portable.matches_metadata_projection(projected)?
    {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "prepared bootstrap extension is not this pool's pure Add authority",
        ));
    }
    for stop in locator.stop_hints() {
        portable.verify_bootstrap_stop(
            &crate::DiskRegistrationIdentity {
                pool_id: stop.pool_id.into(),
                disk_id: stop.disk_id.into(),
                slot_id: stop.slot_id.into(),
                binding_generation: stop.binding_generation,
            },
            stop.operation_id,
            &stop.intent_digest.to_string(),
        )?;
    }
    let effective = portable.effective_config()?;
    let bounded = locator.restrict_config(effective.clone(), &effective)?;
    let hash = portable.hash()?;
    let candidates = candidates(
        locator.select(&bounded).map_err(management_error)?,
        &bounded,
    )?;
    let mut held = Vec::with_capacity(candidates.len());
    for ProbeCandidate { probe, disk } in candidates {
        match io.stat(probe) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
            Ok(_) => (),
        }
        let root = io.open(probe)?;
        if FileIdentity::from_fd(root.directory(root.path())?)? != probe.expected_identity() {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "prepared Add root differs from the exact extension observation",
            ));
        }
        let disk = disk.ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "prepared Add probe is outside its authenticated target",
            )
        })?;
        let marker = marker_path_at(&root, probe.path(), projected)?;
        let authority_hash = marker.as_ref().map(PortableAuthority::hash).transpose()?;
        if authority_hash
            .as_deref()
            .is_some_and(|marker| marker != hash && Some(marker) != intent.origin.predecessor())
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "prepared Add root has an unrelated generation marker",
            ));
        }
        held.push(HeldCandidate {
            disk,
            root,
            authority_hash,
        });
    }
    let evidence = verify_held_generation_from(
        portable.clone(),
        held,
        GenerationEvidenceSource::PreparedRootExtension,
    )?;
    let config = locator.restrict_config(evidence.config, &effective)?;
    portable
        .restrict_io(config)?
        .with_workspace_limits(Some(projected.effective_workspace_limits()))
}

fn verify_held_genesis(
    projected: &ValidatedConfig,
    held: Vec<HeldCandidate>,
) -> Result<RestrictedGenerationEvidence> {
    let mut selected: Option<ValidatedConfig> = None;
    let mut verified_members = Vec::new();
    let mut rejected_members = Vec::new();
    for held in held {
        let result = (|| {
            let saved: ValidatedConfig = held.root.read_json_observed(
                &held.disk.root.join("metadata/pool-membership/config.json"),
                &mut |_| Ok(()),
            )?;
            if !saved.same_genesis_io_authority(projected)
                || selected
                    .as_ref()
                    .is_some_and(|previous| !previous.same_genesis_io_authority(&saved))
            {
                return Err(Error::new(
                    ErrorCode::ConfigConflict,
                    "genesis projection differs from fixed saved registration authority",
                ));
            }
            let disk = saved
                .metadata_members()
                .find(|disk| disk.disk_id == held.disk.disk_id)
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::ConfigConflict,
                        "probe is not a saved genesis voter",
                    )
                })?;
            let expected = identity_at(&saved, disk, &held.root.directory(held.root.path())?)?;
            let stored = decode_seal(
                &held
                    .root
                    .read_bytes(&held.disk.root.join("disk.json"), MAX_METADATA)?,
            )?;
            validate_stored_seal(&stored, &expected)?;
            if selected.is_none() {
                selected = Some(saved);
            }
            Ok(())
        })();
        match result {
            Ok(()) => verified_members.push(held.disk.disk_id),
            Err(error) => rejected_members.push((held.disk.disk_id, error)),
        }
    }
    let Some(config) = selected else {
        return Err(match rejected_members.pop() {
            Some((_, error)) => error.context(
                ErrorCode::RecoveryRequired,
                "retained roots have no verified genesis authority",
            ),
            None => Error::new(
                ErrorCode::RecoveryRequired,
                "retained roots have no verified genesis authority",
            ),
        });
    };
    if verified_members.len() < config.metadata_quorum() {
        return Err(Error::new(
            ErrorCode::MetadataQuorumUnavailable,
            "retained roots cannot satisfy the original fixed genesis quorum",
        ));
    }
    let metadata_members = config.metadata_members().len();
    Ok(RestrictedGenerationEvidence {
        #[cfg(test)]
        authority_hash: object_hash("pool-config", &config)?,
        metadata_members,
        metadata_quorum: config.metadata_quorum(),
        config,
        verified_members,
        rejected_members,
    })
}

#[cfg(test)]
#[path = "../../../../../../../../test/engine/unit/engine/restricted_generation_discovery_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../../../../../test/engine/unit/engine/membership_abort_recovery_tests.rs"]
mod abort_tests;

#[cfg(test)]
#[path = "../../../../../../../../test/engine/unit/engine/membership_root_extension_tests.rs"]
mod root_extension_tests;
