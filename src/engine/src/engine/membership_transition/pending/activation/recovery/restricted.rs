//! Fixed probe set, read-only local evidence. This is not Engine admission.
//! Nothing here reconciles pending stop intents, proves latest publication, or
//! activates an exit generation. Current PortableAuthority supports pure adds.
use super::*;
use crate::engine::disk_lifecycle::bootstrap::{
    BootstrapError, ReadProbe, RestrictedProbes, store::BootstrapStore,
};

#[derive(Debug, error_definition::Error)]
pub(crate) enum RestrictedDiscoveryFailure {
    #[error(transparent)]
    Bootstrap(#[from] BootstrapError),
    #[error(transparent)]
    Verification(#[from] Error),
}

/// A read-only observation, deliberately without Config, State, root handles,
/// admission conversion, or write capability. The target N/Q comes from the
/// authenticated authority; the probe count never changes its denominator.
#[derive(Debug)]
pub(crate) struct RestrictedGenerationEvidence {
    authority_hash: String,
    metadata_members: usize,
    metadata_quorum: usize,
    verified_members: Vec<String>,
    rejected_members: Vec<(String, Error)>,
}
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

impl BootstrapStore {
    /// Failure to load/select returns before the first storage-root syscall.
    /// The caller opens protected system state separately; missing system state
    /// is management-only, never permission to create a locator from old Config.
    pub(crate) fn discover_generation(
        &self,
        config: &ValidatedConfig,
    ) -> std::result::Result<RestrictedGenerationEvidence, RestrictedDiscoveryFailure> {
        discover_store_with_io(self, config, &mut NamedRootIo)
    }
}
impl RestrictedProbes<'_> {
    pub(crate) fn discover_generation(
        self,
        config: &ValidatedConfig,
    ) -> Result<RestrictedGenerationEvidence> {
        discover_with_io(self, config, &mut NamedRootIo)
    }
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
    disk: DiskConfig,
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
        let Some(disk) = disk else {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "restricted binding is absent",
            ));
        };
        if binding.pool_id != config.pool_id
            || binding.slot_id != disk.slot_id
            || binding.binding_generation != disk.binding_generation
            || disk.root.as_os_str() != probe.path().as_os_str()
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "restricted probe cannot be rebound to another projection",
            ));
        }
        candidates.push(ProbeCandidate {
            probe,
            disk: disk.clone(),
        });
        Ok(())
    })?;
    Ok(candidates)
}

fn discover_store_with_io(
    store: &BootstrapStore,
    config: &ValidatedConfig,
    io: &mut impl RootIo,
) -> std::result::Result<RestrictedGenerationEvidence, RestrictedDiscoveryFailure> {
    let locator = store.reload(&config.pool_id)?;
    Ok(discover_with_io(locator.select(config)?, config, io)?)
}

fn discover_with_io(
    probes: RestrictedProbes<'_>,
    config: &ValidatedConfig,
    io: &mut impl RootIo,
) -> Result<RestrictedGenerationEvidence> {
    // Construct the complete immutable candidate list before any root I/O.
    // No fence, authority, archive or Config path can append to this list.
    let candidates = candidates(probes, config)?;
    let mut held = Vec::with_capacity(candidates.len());
    let mut selected: Option<PortableAuthority> = None;
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
        let marker = marker_at(&root, &disk, config)?;
        let authority_hash = marker.as_ref().map(PortableAuthority::hash).transpose()?;
        if let Some(authority) = marker {
            select_authority(&mut selected, authority)?;
        }
        held.push(HeldCandidate {
            disk,
            root,
            authority_hash,
        });
    }
    let portable = selected.ok_or_else(|| {
        Error::new(
            ErrorCode::RecoveryRequired,
            "restricted roots have no generation authority; management recovery is required",
        )
    })?;
    verify_held_generation(portable, held)
}

fn marker_at(
    root: &DiskRoot,
    disk: &DiskConfig,
    projected: &ValidatedConfig,
) -> Result<Option<PortableAuthority>> {
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
        if let Some(file) = root.open_optional_regular(&disk.root.join("disk.json"))? {
            let bytes = read_file_bytes(file, MAX_METADATA)?;
            let value: json_codec::Value = decode_strict(&bytes)?;
            if value.get("format").and_then(json_codec::Value::as_str) == Some("membership-pending")
            {
                // A typed pending seal without a marker supplies no generation
                // evidence. It cannot vote or turn into a fallback projection.
                let _: PendingDiskSeal = decode_strict(&bytes)?;
                return Ok(None);
            }
            let seal = decode_seal(&bytes)?;
            if matches!(seal.format, DiskSealFormat::MembershipGeneration { .. }) {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "active seal lacks its marker",
                ));
            }
        }
        return Ok(None);
    };
    let portable: PortableAuthority = decode_strict(&read_file_bytes(file, MAX_METADATA)?)?;
    if !portable.lineage()?.iter().any(|(_, intent)| {
        projected.same_metadata_membership(intent.origin.config())
            || projected.same_metadata_membership(&intent.target)
    }) {
        return Err(Error::new(
            ErrorCode::ConfigConflict,
            "projection differs from restricted lineage",
        ));
    }
    Ok(Some(portable))
}
fn select_authority(
    selected: &mut Option<PortableAuthority>,
    authority: PortableAuthority,
) -> Result<()> {
    if let Some(previous) = selected {
        let previous_hash = previous.hash()?;
        let hash = authority.hash()?;
        if previous_hash != hash && !authority.contains(&previous_hash)? {
            if !previous.contains(&hash)? {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "restricted roots have conflicting authorities",
                ));
            }
            return Ok(());
        }
    }
    *selected = Some(authority);
    Ok(())
}

fn verify_held_generation(
    portable: PortableAuthority,
    held: Vec<HeldCandidate>,
) -> Result<RestrictedGenerationEvidence> {
    let intent = portable.intent()?;
    let hash = portable.hash()?;
    let transition = object_hash(INTENT_KIND, &intent)?;
    let verified = portable.joint.verify()?;
    let mut config = None;
    let mut evidence = RestrictedGenerationEvidence {
        authority_hash: hash.clone(),
        metadata_members: intent.target.metadata_members().len(),
        metadata_quorum: intent.target.metadata_quorum(),
        verified_members: Vec::new(),
        rejected_members: Vec::new(),
    };
    for held in held {
        let Some(member) = intent
            .target
            .metadata_members()
            .find(|disk| disk.disk_id == held.disk.disk_id)
        else {
            continue;
        };
        let result = (|| {
            if member.slot_id != held.disk.slot_id
                || member.binding_generation != held.disk.binding_generation
                || held.authority_hash.as_deref() != Some(hash.as_str())
            {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "retained member has no exact selected generation",
                ));
            }
            // The authenticated member supplies identity/role, never its address.
            let mut disk = member.clone();
            disk.root = held.disk.root.clone();
            let dependency = verified.dependencies.get(&disk.disk_id).ok_or_else(|| {
                Error::new(
                    ErrorCode::RecoveryRequired,
                    "selected authority lacks member dependency",
                )
            })?;
            let physical = FileIdentity::from_fd(held.root.directory(held.root.path())?)?;
            let identity = &dependency.enrollment.identity;
            if identity != &admission::member_identity(&intent.target, &disk, physical) {
                return Err(Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "retained root differs from joint enrollment",
                ));
            }
            let saved: ValidatedConfig = held.root.read_json_observed(
                &disk.root.join("metadata/pool-membership/config.json"),
                &mut |_| Ok(()),
            )?;
            portable
                .genesis_config()?
                .verify_saved_metadata_membership(&saved)?;
            let parent = held
                .root
                .private_child(&member_directory(&disk, &transition))?;
            let store = joint::store::JointStore::open(
                &held.root,
                &parent,
                identity,
                &transition,
                &portable.joint.proposal_hash,
                false,
            )?;
            store.verify_certificate_read_only(
                &portable.joint.certificate_hash,
                &portable.joint.proposal_hash,
                &disk,
            )?;
            let (state, history) = read_generation_base(
                parent.private_child(&parent.path().join(CONTROL_ARCHIVE_DIRECTORY))?,
                &portable.archive_hash()?,
                &transition,
                &intent,
            )?;
            if config.is_none() {
                config = Some(intent.target.clone().with_generation(Arc::new(Generation {
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
    Ok(evidence)
}

#[cfg(test)]
#[path = "../../../../../../../../test/engine/unit/engine/restricted_generation_discovery_tests.rs"]
mod tests;
