//! Flat, hash-linked generation authority. Old node bytes remain unchanged;
//! successors carry each immutable predecessor node exactly once.
use super::foreign::ForeignCore;
use super::*;
use crate::engine::membership_transition::abort::{AbortAuthorityEvidence, AbortCore};
use crate::engine::membership_transition::backup::BackupCore;
use crate::engine::membership_transition::terminal_resume::ResumeCore;
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct AuthorityNode {
    pub(super) joint: PortableJoint,
    pub(super) archive: ArchiveRoot,
    #[serialization(default, skip_serializing_if = "Option::is_none")]
    registry: Option<registry::RegistryContinuation>,
}

/// The variants have disjoint strict schemas: Add has only joint/archive and
/// Abort requires its exact format. A malformed Abort cannot decode as Add.
/// Untagged serialization preserves every existing Add core byte and hash.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(untagged)]
enum AuthorityCore {
    Add(AuthorityNode),
    Foreign(Box<ForeignCore>),
    Abort(AbortCore),
    Exit(exit::ExitCore),
    TerminalResume(Box<ResumeCore>),
    Readmission(Box<super::super::super::readmission::ReadmissionCore>),
    Backup(Box<BackupCore>),
}

pub(in crate::engine::membership_transition) trait AuthorityResolver {
    fn core_object(&mut self, hash: &str) -> Result<AuthenticatedMetadataObject>;
    fn abort_evidence(&mut self, hash: &str) -> Result<AbortAuthorityEvidence>;
}
impl AuthorityCore {
    fn retained_readmission<'a>(
        &'a self,
        intent: &'a TransitionIntent,
    ) -> Result<Option<&'a super::super::super::readmission::ReadmissionRequest>> {
        match self {
            Self::Readmission(_) => super::super::super::readmission::request(intent).map(Some),
            Self::TerminalResume(core) => Ok(core.retained_request()),
            _ => Ok(None),
        }
    }
    fn stop_intent<'a>(&'a self, intent: &'a TransitionIntent) -> Option<&'a TransitionIntent> {
        match self {
            Self::Exit(_) => Some(intent),
            Self::TerminalResume(core) => Some(core.terminal().intent()),
            Self::Add(_)
            | Self::Foreign(_)
            | Self::Abort(_)
            | Self::Readmission(_)
            | Self::Backup(_) => None,
        }
    }
    fn archive(&self) -> &ArchiveRoot {
        match self {
            Self::Add(node) => &node.archive,
            Self::Foreign(core) => &core.node().archive,
            Self::Abort(core) => &core.archive,
            Self::Exit(core) => &core.archive,
            Self::TerminalResume(core) => core.archive(),
            Self::Readmission(core) => core.archive(),
            Self::Backup(core) => core.archive(),
        }
    }
    fn intent(&self, evidence: Option<&AbortAuthorityEvidence>) -> Result<TransitionIntent> {
        match (self, evidence) {
            (Self::Add(node), None) => node.intent(),
            (Self::Foreign(core), None) => core.intent(),
            (Self::Exit(core), None) => core.intent(),
            (Self::TerminalResume(core), None) => core.intent(),
            (Self::Readmission(core), None) => core.intent(),
            (Self::Backup(core), None) => core.intent(),
            (Self::Abort(core), Some(evidence)) => evidence.verify(core),
            _ => Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "authority evidence does not exactly cover Abort cores",
            )),
        }
    }
    fn effective_config(&self, intent: &TransitionIntent) -> Result<ValidatedConfig> {
        match self {
            Self::Add(_)
            | Self::Foreign(_)
            | Self::Exit(_)
            | Self::TerminalResume(_)
            | Self::Readmission(_) => Ok(intent.target.clone()),
            Self::Abort(_) => Ok(intent.origin.config().clone()),
            Self::Backup(core) => core.effective_config(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PortableAuthority {
    head: AuthorityCore,
    predecessors: BTreeMap<String, AuthorityCore>,
    evidence: BTreeMap<String, AbortAuthorityEvidence>,
}

#[derive(Serialize, Deserialize)]
enum ChainFormat {
    #[serialization(rename = "membership-generation-chain")]
    MembershipGenerationChain,
}

#[derive(Serialize)]
struct ChainRef<'a> {
    format: ChainFormat,
    head: &'a AuthorityCore,
    predecessors: &'a BTreeMap<String, AuthorityCore>,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct Chain {
    format: ChainFormat,
    head: AuthorityCore,
    predecessors: BTreeMap<String, AuthorityCore>,
}

#[derive(Serialize, Deserialize)]
enum EvidenceFormat {
    #[serialization(rename = "membership-generation-evidence-chain")]
    DetachedEvidence,
}
#[derive(Serialize)]
struct EvidenceRef<'a> {
    format: EvidenceFormat,
    head: &'a AuthorityCore,
    predecessors: &'a BTreeMap<String, AuthorityCore>,
    evidence: &'a BTreeMap<String, AbortAuthorityEvidence>,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct EvidenceChain {
    format: EvidenceFormat,
    head: AuthorityCore,
    predecessors: BTreeMap<String, AuthorityCore>,
    evidence: BTreeMap<String, AbortAuthorityEvidence>,
}

#[derive(Deserialize)]
#[serialization(untagged)]
enum StoredAuthority {
    Evidence(EvidenceChain),
    Chain(Chain),
    Genesis(AuthorityCore),
}

impl Serialize for PortableAuthority {
    fn serialize<S: data_serialization::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        if !self.evidence.is_empty() {
            EvidenceRef {
                format: EvidenceFormat::DetachedEvidence,
                head: &self.head,
                predecessors: &self.predecessors,
                evidence: &self.evidence,
            }
            .serialize(serializer)
        } else if self.predecessors.is_empty() {
            self.head.serialize(serializer)
        } else {
            ChainRef {
                format: ChainFormat::MembershipGenerationChain,
                head: &self.head,
                predecessors: &self.predecessors,
            }
            .serialize(serializer)
        }
    }
}

impl<'de> Deserialize<'de> for PortableAuthority {
    fn deserialize<D: data_serialization::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let authority = match StoredAuthority::deserialize(deserializer)? {
            StoredAuthority::Evidence(EvidenceChain {
                format: EvidenceFormat::DetachedEvidence,
                head,
                predecessors,
                evidence,
            }) => {
                if evidence.is_empty() {
                    return Err(data_serialization::de::Error::custom(
                        "detached authority requires Abort evidence",
                    ));
                }
                Self {
                    head,
                    predecessors,
                    evidence,
                }
            }
            StoredAuthority::Genesis(head) => Self {
                head,
                predecessors: BTreeMap::new(),
                evidence: BTreeMap::new(),
            },
            StoredAuthority::Chain(Chain {
                format: ChainFormat::MembershipGenerationChain,
                head,
                predecessors,
            }) => {
                if predecessors.is_empty() {
                    return Err(data_serialization::de::Error::custom(
                        "generation chain requires a predecessor",
                    ));
                }
                Self {
                    head,
                    predecessors,
                    evidence: BTreeMap::new(),
                }
            }
        };
        authority
            .lineage()
            .map_err(data_serialization::de::Error::custom)?;
        Ok(authority)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AdditionKind {
    Empty,
    Foreign,
}
impl AuthorityNode {
    fn intent(&self) -> Result<TransitionIntent> {
        self.intent_kind(AdditionKind::Empty)
    }
    pub(super) fn foreign_intent(&self) -> Result<TransitionIntent> {
        self.intent_kind(AdditionKind::Foreign)
    }
    fn intent_kind(&self, kind: AdditionKind) -> Result<TransitionIntent> {
        let verified = self.joint.verify()?;
        if verified.intent.foreign.is_some() != (kind == AdditionKind::Foreign) {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "foreign recipe must be carried by its distinct authority kind",
            ));
        }
        verified.intent.verify_target()?;
        self.archive.verify_scope(&verified.intent)?;
        if verified.intent.effect.is_some()
            || matches!(
                verified.intent.origin,
                Origin::TerminalResume { .. }
                    | Origin::Readmission { .. }
                    | Origin::BackupCutover { .. }
            )
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "non-addition control evidence cannot authorize ordinary Joint activation",
            ));
        }
        if object_hash("membership-control-archive", &self.archive)?
            != verified.proposal.control_archive_root
        {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "generation archive differs from joint authority",
            ));
        }
        let original = verified.intent.origin.config();
        let target = &verified.intent.target;
        if !admission::same_settings(original, target)
            || original.disks.iter().any(|old| {
                !target
                    .disks
                    .iter()
                    .any(|new| admission::same_disk(old, new))
            })
            || target.disks.len() <= original.disks.len()
            || target
                .disks
                .iter()
                .filter(|disk| !original.disks.iter().any(|old| old.disk_id == disk.disk_id))
                .any(|disk| disk.binding_generation != 1)
        {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "generation authority is not a pure addition",
            ));
        }
        let registry = self
            .registry
            .as_ref()
            .map(|registry| registry.verify(&verified.intent, &verified.intent.target))
            .transpose()?;
        crate::engine::membership_transition::complete_chain::verify_authority_chain(
            &verified.intent,
            registry.as_ref(),
        )?;
        Ok(verified.intent)
    }
    pub(super) fn verify_membership(
        &self,
        candidate: &Candidate,
        intent: &TransitionIntent,
        hash: &str,
    ) -> Result<()> {
        match &self.registry {
            Some(registry) => registry
                .verify(intent, &intent.target)?
                .verify_lifecycle_candidate(candidate, hash),
            None => intent
                .target
                .verify_original_candidate_membership(candidate),
        }
    }
}

impl PortableAuthority {
    /// Add-only resources must never be inferred from an Abort authority.
    pub(super) fn add_joint(&self) -> Result<&PortableJoint> {
        match &self.head {
            AuthorityCore::Add(node) => Ok(&node.joint),
            AuthorityCore::Exit(core) => Ok(&core.joint),
            AuthorityCore::Abort(_)
            | AuthorityCore::Foreign(_)
            | AuthorityCore::TerminalResume(_)
            | AuthorityCore::Readmission(_)
            | AuthorityCore::Backup(_) => Err(Error::new(
                ErrorCode::RecoveryRequired,
                "this authority has no Joint addition certificate",
            )),
        }
    }
    pub(in crate::engine::membership_transition) fn abort_status(
        &self,
    ) -> Result<super::super::super::abort::MembershipAbortStatus> {
        let AuthorityCore::Abort(core) = &self.head else {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "authority is not an Abort",
            ));
        };
        self.evidence[&self.hash()?].status(core)
    }

    pub(in crate::engine::membership_transition) fn verify_abort_member(
        &self,
        root: &DiskRoot,
        disk: &DiskConfig,
    ) -> Result<()> {
        self.lineage()?;
        match &self.head {
            AuthorityCore::Abort(core) => {
                self.evidence[&self.hash()?].verify_member(core, root, disk)
            }
            AuthorityCore::Add(_)
            | AuthorityCore::Foreign(_)
            | AuthorityCore::Exit(_)
            | AuthorityCore::TerminalResume(_)
            | AuthorityCore::Readmission(_)
            | AuthorityCore::Backup(_) => Err(Error::new(
                ErrorCode::ConfigConflict,
                "expected Abort authority",
            )),
        }
    }

    pub(in crate::engine) fn is_foreign(&self) -> bool {
        matches!(self.head, AuthorityCore::Foreign(_))
    }
    pub(in crate::engine::membership_transition) fn first_sequence(&self) -> Result<u64> {
        let intent = self.intent()?;
        if self.is_foreign() {
            return intent.first_sequence();
        }
        intent
            .fence_sequence
            .checked_add(1)
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimit, "activation sequence exhausted"))
    }
    pub(in crate::engine) fn is_exit(&self) -> bool {
        matches!(self.head, AuthorityCore::Exit(_))
    }
    pub(in crate::engine) fn exit_registry(&self) -> Option<&crate::DiskRegistryCommitment> {
        self.registry_at(&self.head)
            .expect("invariant: admitted authority lineage")
    }

    fn registry_at<'a>(
        &'a self,
        node: &'a AuthorityCore,
    ) -> Result<Option<&'a crate::DiskRegistryCommitment>> {
        match node {
            AuthorityCore::Add(node) => Ok(node
                .registry
                .as_ref()
                .map(registry::RegistryContinuation::registry)),
            AuthorityCore::Exit(core) => Ok(Some(core.registry())),
            AuthorityCore::TerminalResume(core) => Ok(Some(core.registry())),
            AuthorityCore::Readmission(core) => Ok(Some(core.registry())),
            AuthorityCore::Backup(_) => Err(BackupCore::activation_required()),
            AuthorityCore::Foreign(_) => Err(ForeignCore::activation_required()),
            AuthorityCore::Abort(_) => {
                let hash = object_hash(AUTHORITY_KIND, node)?;
                let intent = node.intent(self.evidence.get(&hash))?;
                match intent.origin.predecessor() {
                    Some(parent) => self.registry_at(self.node(parent)?),
                    None => Ok(None),
                }
            }
        }
    }

    pub(in crate::engine) fn verified_registry(
        &self,
    ) -> Result<Option<crate::VerifiedDiskRegistry>> {
        self.lineage()?;
        self.registry_for_node(&self.head)
    }

    fn registry_for_node(
        &self,
        node: &AuthorityCore,
    ) -> Result<Option<crate::VerifiedDiskRegistry>> {
        match node {
            AuthorityCore::Add(node) => node
                .registry
                .as_ref()
                .map(|registry| {
                    let intent = node.intent()?;
                    registry.verify(&intent, &intent.target)
                })
                .transpose(),
            AuthorityCore::Exit(core) => core.verified_registry().map(Some),
            AuthorityCore::TerminalResume(core) => core.verified_registry().map(Some),
            AuthorityCore::Readmission(core) => core.verified_registry().map(Some),
            AuthorityCore::Backup(_) => Err(BackupCore::activation_required()),
            AuthorityCore::Foreign(_) => Err(ForeignCore::activation_required()),
            AuthorityCore::Abort(_) => {
                let hash = object_hash(AUTHORITY_KIND, node)?;
                let intent = node.intent(self.evidence.get(&hash))?;
                match intent.origin.predecessor() {
                    Some(hash) => self.registry_for_node(self.node(hash)?),
                    None => Ok(None),
                }
            }
        }
    }
    pub(in crate::engine) fn verify_registry_source(&self, source: &State) -> Result<()> {
        match &self.head {
            AuthorityCore::Foreign(core) => {
                if let Some(registry) = &core.node().registry {
                    registry.verify_source(&core.intent()?, source)?;
                }
            }
            AuthorityCore::Add(node) => {
                if let Some(registry) = &node.registry {
                    registry.verify_source(&self.intent()?, source)?;
                }
            }
            AuthorityCore::Exit(core) => {
                core.registry_from_source(source)?;
            }
            AuthorityCore::TerminalResume(core) => core.verify_source(source)?,
            AuthorityCore::Readmission(core) => core.verify_source(source)?,
            AuthorityCore::Backup(core) => core.verify_source(source)?,
            AuthorityCore::Abort(_) => (),
        }
        Ok(())
    }

    pub(in crate::engine) fn restrict_io(
        &self,
        config: ValidatedConfig,
    ) -> Result<ValidatedConfig> {
        if self.is_backup() {
            return Err(BackupCore::activation_required());
        }
        if self.is_foreign() {
            return Err(ForeignCore::activation_required());
        }
        let forbidden = self.forbidden_disk_ids()?;
        if forbidden.is_empty() {
            return Ok(config);
        }
        let allowed = config
            .io_disks()
            .filter(|disk| !forbidden.contains(&disk.disk_id))
            .map(|disk| disk.disk_id.clone())
            .collect();
        config.restrict_io_disks(allowed)
    }

    pub(super) fn prepare_namespace_activation(
        &self,
        source: &State,
        previous: Option<&crate::engine::disk_registry::current::CurrentNamespaces>,
    ) -> Result<crate::engine::disk_registry::current::PreparedCurrentNamespaces> {
        if self.is_backup() {
            return Err(BackupCore::activation_required());
        }
        if self.is_foreign() {
            return Err(ForeignCore::activation_required());
        }
        let intent = self.intent()?;
        if let AuthorityCore::TerminalResume(core) = &self.head {
            return core.current_namespaces(source, previous);
        }
        if let AuthorityCore::Exit(core) = &self.head {
            return core.current_namespaces(source, previous);
        }
        if let AuthorityCore::Readmission(core) = &self.head {
            core.verify_source(source)?;
            let registry = core.verified_registry()?;
            let request = super::super::super::readmission::request(&intent)?;
            return previous
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::RecoveryRequired,
                        "readmission lacks its current namespace source",
                    )
                })?
                .prepare_readmission(
                    source,
                    &intent
                        .boundary
                        .as_ref()
                        .expect("invariant: readmission source")
                        .snapshot_hash,
                    &registry,
                    &request.target,
                );
        }
        if let AuthorityCore::Add(node) = &self.head
            && let Some(registry) = &node.registry
        {
            registry.verify_source(&intent, source)?;
        }
        let registry = self.verified_registry()?.ok_or_else(|| {
            Error::new(
                ErrorCode::PublicationInvalid,
                "namespace continuation lacks a registry",
            )
        })?;
        let snapshot = intent.boundary.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::PublicationInvalid,
                "namespace continuation lacks its source",
            )
        })?;
        previous
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::RecoveryRequired,
                    "namespace continuation lacks the fully replayed predecessor view",
                )
            })?
            .prepare_continuation(source, &snapshot.snapshot_hash, &registry)
    }

    pub(super) fn activation_mask_hash(&self) -> Result<Option<String>> {
        if let Some(core) = self.terminal_resume_core() {
            return core.activation_mask_hash();
        }
        let Some(core) = self.exit_core() else {
            return Ok(None);
        };
        let registry = core.verified_registry()?;
        Ok(
            match &registry
                .pending_exit()
                .expect("invariant: verified exit request")
                .effect
            {
                crate::RegistryExitEffect::Disable => None,
                crate::RegistryExitEffect::Detach {
                    occurrence_mask_hash,
                } => Some(occurrence_mask_hash.clone()),
            },
        )
    }

    pub(in crate::engine) fn exit_core(&self) -> Option<&exit::ExitCore> {
        match &self.head {
            AuthorityCore::Exit(core) => Some(core),
            _ => None,
        }
    }
    pub(in crate::engine::membership_transition) fn readmission_status(
        &self,
        operation: OperationId,
        phase: DiskReadmissionPhase,
    ) -> Result<Option<DiskReadmissionStatus>> {
        for (hash, intent) in self.lineage()? {
            if intent.operation_id == operation
                && matches!(intent.origin, Origin::Readmission { .. })
            {
                let AuthorityCore::Readmission(core) = self.node(&hash)? else {
                    unreachable!("invariant: readmission intent belongs to a readmission core");
                };
                return super::super::super::readmission::status(
                    &intent,
                    phase,
                    core.source_topology_generation(),
                )
                .map(Some);
            }
        }
        Ok(None)
    }
    pub(in crate::engine) fn readmission_core(
        &self,
    ) -> Option<&super::super::super::readmission::ReadmissionCore> {
        match &self.head {
            AuthorityCore::Readmission(core) => Some(core),
            _ => None,
        }
    }
    pub(in crate::engine::membership_transition) fn target_predecessor_seal(
        &self,
        disk_id: &str,
    ) -> Result<Option<DiskSeal>> {
        if let Some(core) = self.readmission_core() {
            return core.target_predecessor_seal(disk_id);
        }
        Ok(self
            .terminal_resume_core()
            .and_then(|core| core.retained_request())
            .filter(|request| request.target.disk_id == disk_id)
            .map(|request| request.original_seal.clone()))
    }
    pub(in crate::engine::membership_transition) fn permits_retained_prior_marker(
        &self,
        disk_id: &str,
        hash: &str,
    ) -> Result<bool> {
        if let Some(core) = self.readmission_core() {
            return core.permits_prior_marker(disk_id, hash);
        }
        Ok(self.target_predecessor_seal(disk_id)?.is_some_and(|seal| {
            matches!(seal.format,
            DiskSealFormat::MembershipGeneration { authority_hash } if authority_hash == hash)
        }))
    }
    pub(in crate::engine) fn io_admission_epoch(&self, disk_id: &str) -> Result<Option<String>> {
        let mut epoch = None;
        for (hash, intent) in self.lineage()? {
            if let AuthorityCore::Backup(core) = self.node(&hash)?
                && let Some(selected) = core.writer_epoch(&hash, disk_id)?
            {
                epoch = Some(selected);
            }
            if self
                .node(&hash)?
                .retained_readmission(&intent)?
                .is_some_and(|request| request.target.disk_id == disk_id)
            {
                epoch = Some(object_hash(INTENT_KIND, &intent)?);
            }
        }
        Ok(epoch)
    }
    /// Resolve one currently active epoch only from the complete authenticated
    /// lineage. A historical readmission cannot revive a later Disabled/Detached root.
    pub(in crate::engine::membership_transition) fn readmission_epoch(
        &self,
        identity: &crate::DiskRegistrationIdentity,
        epoch: &str,
    ) -> Result<(
        TransitionIntent,
        super::super::super::readmission::ReadmissionRequest,
    )> {
        let registry = self.verified_registry()?.ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "readmission recovery has no registry",
            )
        })?;
        if registry.registration(identity)? != Some(crate::RegistrationMode::Enabled)
            || self.io_admission_epoch(&identity.disk_id)?.as_deref() != Some(epoch)
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "readmission recovery does not name the current Enabled epoch",
            ));
        }
        for (hash, intent) in self.lineage()? {
            if let Some(request) = self.node(&hash)?.retained_readmission(&intent)?
                && request.target == *identity
                && object_hash(INTENT_KIND, &intent)? == epoch
            {
                let request = request.clone();
                return Ok((intent, request));
            }
        }
        Err(Error::new(
            ErrorCode::RecoveryRequired,
            "readmission epoch is absent from authenticated lineage",
        ))
    }

    pub(in crate::engine) fn retired_io_epochs(
        &self,
        disk_id: &str,
    ) -> Result<BTreeSet<Option<String>>> {
        let mut retired = BTreeSet::new();
        for (hash, intent) in self.lineage()? {
            if let AuthorityCore::Backup(core) = self.node(&hash)?
                && core.writer_epoch(&hash, disk_id)?.is_some()
            {
                // These are fresh selected identities, not Genesis members.
                // Never let a separately parsed projection mint Genesis I/O.
                retired.insert(None);
            }
            if let Some(request) = self.node(&hash)?.retained_readmission(&intent)?
                && request.target.disk_id == disk_id
            {
                retired.insert(request.previous_admission.clone());
            }
        }
        Ok(retired)
    }

    pub(in crate::engine) fn backup_writer_target_ids(
        &self,
        projected: &ValidatedConfig,
    ) -> Result<BTreeSet<String>> {
        let core = self
            .backup_core()
            .ok_or_else(BackupCore::activation_required)?;
        let expected = core.effective_config()?;
        if object_hash("backup-effective-config", projected)?
            != object_hash("backup-effective-config", &expected)?
            || !projected.same_genesis_io_authority(&expected)
        {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "Backup writer projection changes its exact original authority or selected roots",
            ));
        }
        core.writer_targets()
    }

    pub(in crate::engine) fn historical_bootstrap_stops(
        &self,
    ) -> Result<Vec<(crate::DiskRegistrationIdentity, OperationId, String)>> {
        let mut stops = Vec::new();
        for (hash, intent) in self.lineage()? {
            let Some(stopped) = self.node(&hash)?.stop_intent(&intent) else {
                continue;
            };
            let effect = stopped
                .effect
                .as_ref()
                .expect("invariant: verified stop effect");
            stops.push((
                effect.identity(&stopped.origin.config().pool_id),
                stopped.operation_id,
                object_hash(INTENT_KIND, stopped)?,
            ));
        }
        Ok(stops)
    }

    pub(in crate::engine) fn bootstrap_stops(
        &self,
    ) -> Result<Vec<(crate::DiskRegistrationIdentity, OperationId, String)>> {
        let mut stops = BTreeMap::new();
        for (hash, intent) in self.lineage()? {
            let node = self.node(&hash)?;
            if let Some(stopped) = node.stop_intent(&intent) {
                let effect = stopped
                    .effect
                    .as_ref()
                    .expect("invariant: verified stop effect");
                let binding = effect.identity(&stopped.origin.config().pool_id);
                if let Some((previous, _, _)) = stops.get(&binding.disk_id)
                    && previous != &binding
                {
                    return Err(Error::new(
                        ErrorCode::PublicationInvalid,
                        "stopped logical disk is rebound in authority lineage",
                    ));
                }
                // Disable followed by Deregister closes the same I/O lifetime.
                // Only authenticated readmission below starts a new lifetime.
                stops.entry(binding.disk_id.clone()).or_insert((
                    binding,
                    stopped.operation_id,
                    object_hash(INTENT_KIND, stopped)?,
                ));
            }
            if let Some(request) = node.retained_readmission(&intent)? {
                stops.remove(&request.target.disk_id);
            }
        }
        Ok(stops.into_values().collect())
    }

    pub(in crate::engine) fn forbidden_disk_ids(&self) -> Result<BTreeSet<String>> {
        Ok(self
            .bootstrap_stops()?
            .into_iter()
            .map(|(binding, _, _)| binding.disk_id)
            .collect())
    }
    pub(in crate::engine) fn verify_bootstrap_stop(
        &self,
        binding: &crate::DiskRegistrationIdentity,
        operation_id: OperationId,
        intent_hash: &str,
    ) -> Result<()> {
        if self
            .historical_bootstrap_stops()?
            .iter()
            .any(|(identity, operation, hash)| {
                identity == binding && *operation == operation_id && hash == intent_hash
            })
        {
            Ok(())
        } else {
            Err(Error::new(
                ErrorCode::RecoveryRequired,
                "bootstrap stop has no exact authenticated exit authority",
            ))
        }
    }
    pub(in crate::engine) fn verify_bootstrap_stop_epoch(
        &self,
        binding: &crate::DiskRegistrationIdentity,
        operation: OperationId,
        transition: &str,
        epoch: Option<&str>,
    ) -> Result<()> {
        let mut epochs = BTreeMap::new();
        for (hash, intent) in self.lineage()? {
            let node = self.node(&hash)?;
            if let Some(stopped) = node.stop_intent(&intent) {
                let effect = stopped
                    .effect
                    .as_ref()
                    .expect("invariant: verified stop effect");
                if effect.identity(&stopped.origin.config().pool_id) == *binding
                    && stopped.operation_id == operation
                    && object_hash(INTENT_KIND, stopped)? == transition
                {
                    if epochs.get(&binding.disk_id).map(String::as_str) == epoch {
                        return Ok(());
                    }
                    return Err(Error::new(
                        ErrorCode::RecoveryRequired,
                        "bootstrap stop closes another I/O epoch",
                    ));
                }
            }
            if let Some(request) = node.retained_readmission(&intent)? {
                epochs.insert(
                    request.target.disk_id.clone(),
                    object_hash(INTENT_KIND, &intent)?,
                );
            }
        }
        Err(Error::new(
            ErrorCode::RecoveryRequired,
            "bootstrap stop epoch lacks exact authenticated authority",
        ))
    }

    pub(in crate::engine) fn verify_readmission_hint(
        &self,
        identity: &crate::DiskRegistrationIdentity,
        operation: OperationId,
        transition: &str,
        stopped: &str,
    ) -> Result<()> {
        for (hash, intent) in self.lineage()? {
            if let Some(request) = self.node(&hash)?.retained_readmission(&intent)?
                && request.target == *identity
                && intent.operation_id == operation
                && object_hash(INTENT_KIND, &intent)? == transition
                && request.stopped_transition == stopped
            {
                return Ok(());
            }
        }
        Err(Error::new(
            ErrorCode::RecoveryRequired,
            "bootstrap readmission has no exact authenticated authority",
        ))
    }

    pub(in crate::engine::membership_transition) fn is_backup(&self) -> bool {
        matches!(self.head, AuthorityCore::Backup(_))
    }
    pub(super) fn backup_core(&self) -> Option<&BackupCore> {
        match &self.head {
            AuthorityCore::Backup(core) => Some(core),
            _ => None,
        }
    }
    pub(in crate::engine::membership_transition) fn is_abort(&self) -> bool {
        matches!(self.head, AuthorityCore::Abort(_))
    }
    pub(in crate::engine::membership_transition) fn effective_config(
        &self,
    ) -> Result<ValidatedConfig> {
        let intent = self.intent()?;
        self.head.effective_config(&intent)
    }

    pub(super) fn workspace_root_aliases(&self) -> Result<WorkspaceRootAliases> {
        let mut roots: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
        // These names are authenticated historical locators, not permissions to
        // open an old root. Consumers must project onto the current held binding.
        for (hash, intent) in self.lineage()? {
            let effective = self.node(&hash)?.effective_config(&intent)?;
            for config in [intent.origin.config(), &effective] {
                for disk in &config.disks {
                    roots
                        .entry((
                            disk.disk_id.clone(),
                            disk.slot_id.clone(),
                            disk.binding_generation,
                        ))
                        .or_default()
                        .insert(disk.root.clone());
                }
            }
        }
        Ok(roots)
    }
    pub(super) fn data_dependency(&self, disk_id: &str) -> Result<Option<data::DataDependency>> {
        // The lineage proves additions are permanent and identities never change.
        self.lineage()?;
        for node in std::iter::once(&self.head).chain(self.predecessors.values()) {
            if let AuthorityCore::TerminalResume(core) = node {
                if let Some(dependency) = core.data_dependency(disk_id)? {
                    return Ok(Some(dependency));
                }
                continue;
            }
            let AuthorityCore::Add(node) = node else {
                continue;
            };
            let mut verified = node.joint.verify()?;
            if let Some(dependency) = verified.data_disks.remove(disk_id) {
                return Ok(Some(dependency));
            }
        }
        Ok(None)
    }
    pub(in crate::engine::membership_transition) fn load(
        hash: &str,
        resolver: &mut impl AuthorityResolver,
    ) -> Result<Self> {
        let head: AuthorityCore = json_codec::from_value(resolver.core_object(hash)?.into_value())?;
        let mut predecessors = BTreeMap::new();
        let mut evidence = BTreeMap::new();
        if matches!(head, AuthorityCore::Abort(_)) {
            evidence.insert(hash.to_owned(), resolver.abort_evidence(hash)?);
        }
        let mut next = head
            .intent(evidence.get(hash))?
            .origin
            .predecessor()
            .map(str::to_owned);
        while let Some(hash) = next {
            if predecessors.contains_key(&hash) {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "cyclic archived membership lineage",
                ));
            }
            let node: AuthorityCore =
                json_codec::from_value(resolver.core_object(&hash)?.into_value())?;
            if matches!(node, AuthorityCore::Abort(_)) {
                evidence.insert(hash.clone(), resolver.abort_evidence(&hash)?);
            }
            next = node
                .intent(evidence.get(&hash))?
                .origin
                .predecessor()
                .map(str::to_owned);
            predecessors.insert(hash, node);
        }
        let authority = Self {
            head,
            predecessors,
            evidence,
        };
        authority.lineage()?;
        Ok(authority)
    }

    pub(in crate::engine::membership_transition) fn node_objects(
        &self,
    ) -> Result<Vec<PreparedMetadataObject>> {
        std::iter::once(&self.head)
            .chain(self.predecessors.values())
            .map(|node| PreparedMetadataObject::new(AUTHORITY_KIND, node, MAX_METADATA))
            .collect()
    }

    pub(in crate::engine::membership_transition) fn archives(
        &self,
    ) -> Result<Vec<(ArchiveRoot, TransitionIntent)>> {
        let head_hash = self.hash()?;
        self.lineage()?
            .into_iter()
            .map(|(hash, intent)| {
                let node = if hash == head_hash {
                    &self.head
                } else {
                    self.predecessors
                        .get(&hash)
                        .expect("invariant: verified predecessor node")
                };
                let archived_intent = match node {
                    AuthorityCore::TerminalResume(core) => core.archive_intent().clone(),
                    _ => intent,
                };
                Ok((node.archive().clone(), archived_intent))
            })
            .collect()
    }

    pub(super) fn new(
        joint: PortableJoint,
        archive: ArchiveRoot,
        predecessor: Option<&Self>,
        registry: Option<registry::RegistryContinuation>,
    ) -> Result<Self> {
        let mut predecessors = BTreeMap::new();
        let mut evidence = BTreeMap::new();
        if let Some(previous) = predecessor {
            predecessors = previous.predecessors.clone();
            predecessors.insert(previous.hash()?, previous.head.clone());
            evidence = previous.evidence.clone();
        }
        let authority = Self {
            head: {
                let foreign = joint.verify()?.intent.foreign.is_some();
                let node = AuthorityNode {
                    joint,
                    archive,
                    registry,
                };
                if foreign {
                    AuthorityCore::Foreign(Box::new(ForeignCore::new(node)?))
                } else {
                    AuthorityCore::Add(node)
                }
            },
            predecessors,
            evidence,
        };
        authority.lineage()?;
        Ok(authority)
    }

    /// The fixed-Genesis Backup core is a separate native kind. Parsing or
    /// hashing it does not admit Config, Current namespaces or writer resources.
    pub(in crate::engine::membership_transition) fn new_backup(core: BackupCore) -> Result<Self> {
        let authority = Self {
            head: AuthorityCore::Backup(Box::new(core)),
            predecessors: BTreeMap::new(),
            evidence: BTreeMap::new(),
        };
        authority.lineage()?;
        Ok(authority)
    }

    pub(super) fn new_exit(core: exit::ExitCore, predecessor: Option<&Self>) -> Result<Self> {
        let mut predecessors = BTreeMap::new();
        let mut evidence = BTreeMap::new();
        if let Some(previous) = predecessor {
            predecessors = previous.predecessors.clone();
            predecessors.insert(previous.hash()?, previous.head.clone());
            evidence = previous.evidence.clone();
        }
        let authority = Self {
            head: AuthorityCore::Exit(core),
            predecessors,
            evidence,
        };
        authority.lineage()?;
        Ok(authority)
    }

    pub(in crate::engine::membership_transition) fn new_terminal_resume(
        core: ResumeCore,
    ) -> Result<Self> {
        let previous = core.terminal().predecessor();
        let mut predecessors = BTreeMap::new();
        let mut evidence = BTreeMap::new();
        if let Some(previous) = previous {
            predecessors = previous.predecessors.clone();
            predecessors.insert(previous.hash()?, previous.head.clone());
            evidence = previous.evidence.clone();
        }
        let authority = Self {
            head: AuthorityCore::TerminalResume(Box::new(core)),
            predecessors,
            evidence,
        };
        authority.lineage()?;
        Ok(authority)
    }

    pub(in crate::engine::membership_transition) fn terminal_resume_core(
        &self,
    ) -> Option<&ResumeCore> {
        match &self.head {
            AuthorityCore::TerminalResume(core) => Some(core),
            _ => None,
        }
    }

    pub(in crate::engine::membership_transition) fn archive_intent(
        &self,
    ) -> Result<TransitionIntent> {
        match &self.head {
            AuthorityCore::TerminalResume(core) => Ok(core.archive_intent().clone()),
            _ => self.intent(),
        }
    }

    pub(in crate::engine::membership_transition) fn read_archive_base(
        &self,
        root: DiskRoot,
    ) -> Result<(State, BTreeMap<u64, String>)> {
        let intent = self.archive_intent()?;
        read_generation_base(
            root,
            &self.archive_hash()?,
            &object_hash(INTENT_KIND, &intent)?,
            &intent,
        )
    }

    pub(in crate::engine::membership_transition) fn archive_transition(&self) -> Result<String> {
        Ok(object_hash(INTENT_KIND, &self.archive_intent()?)?)
    }

    pub(in crate::engine::membership_transition) fn new_readmission(
        core: super::super::super::readmission::ReadmissionCore,
        previous: &Self,
    ) -> Result<Self> {
        let mut predecessors = previous.predecessors.clone();
        predecessors.insert(previous.hash()?, previous.head.clone());
        let authority = Self {
            head: AuthorityCore::Readmission(Box::new(core)),
            predecessors,
            evidence: previous.evidence.clone(),
        };
        authority.lineage()?;
        Ok(authority)
    }

    pub(crate) fn hash(&self) -> Result<String> {
        Ok(object_hash(AUTHORITY_KIND, &self.head)?)
    }

    pub(in crate::engine::membership_transition) fn archive_hash(&self) -> Result<String> {
        Ok(object_hash(
            "membership-control-archive",
            self.head.archive(),
        )?)
    }

    pub(super) fn lineage(&self) -> Result<Vec<(String, TransitionIntent)>> {
        let mut chain = Vec::new();
        let mut visited = BTreeSet::new();
        let mut node = &self.head;
        loop {
            let hash = object_hash(AUTHORITY_KIND, node)?;
            if !visited.insert(hash.clone()) {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "cyclic membership authority",
                ));
            }
            let intent = node.intent(self.evidence.get(&hash))?;
            let predecessor = intent.origin.predecessor().map(str::to_owned);
            chain.push((hash, intent));
            let Some(predecessor) = predecessor else {
                break;
            };
            node = self.predecessors.get(&predecessor).ok_or_else(|| {
                Error::new(
                    ErrorCode::ObjectCorrupt,
                    "membership predecessor is missing",
                )
            })?;
            if object_hash(AUTHORITY_KIND, node)? != predecessor {
                return Err(Error::new(
                    ErrorCode::ObjectCorrupt,
                    "membership predecessor hash differs",
                ));
            }
            let previous = node.intent(self.evidence.get(&predecessor))?;
            let current = &chain
                .last()
                .expect("invariant: current authority appended")
                .1;
            if !same_config_projection(current.origin.config(), &node.effective_config(&previous)?)
                || current
                    .boundary
                    .as_ref()
                    .is_none_or(|boundary| boundary.sequence <= previous.fence_sequence)
            {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "successor does not extend its exact predecessor",
                ));
            }
        }
        // Registry ancestry is checked separately from membership ancestry. A
        // successor cannot silently re-enable or resurrect a historical binding.
        for (hash, intent) in &chain {
            let node = self.node(hash)?;
            let previous = intent
                .origin
                .predecessor()
                .map(|parent| self.node(parent).and_then(|node| self.registry_at(node)))
                .transpose()?
                .flatten();
            let carried = match node {
                AuthorityCore::Foreign(core) => core
                    .node()
                    .registry
                    .as_ref()
                    .map(registry::RegistryContinuation::parent),
                AuthorityCore::Add(node) => node
                    .registry
                    .as_ref()
                    .map(registry::RegistryContinuation::parent),
                AuthorityCore::Exit(core) => core.registry_parent(),
                AuthorityCore::TerminalResume(core) => core
                    .terminal()
                    .predecessor()
                    .and_then(PortableAuthority::exit_registry),
                AuthorityCore::Abort(_) => previous,
                AuthorityCore::Readmission(core) => Some(core.registry_parent()),
                AuthorityCore::Backup(_) => None,
            };
            if carried != previous {
                return Err(Error::new(
                    ErrorCode::PublicationInvalid,
                    "membership successor does not continue its exact registry predecessor",
                ));
            }
        }
        if visited.len() != self.predecessors.len() + 1 {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "membership lineage contains unrelated nodes",
            ));
        }
        let required = std::iter::once(&self.head)
            .chain(self.predecessors.values())
            .filter(|node| matches!(node, AuthorityCore::Abort(_)))
            .count();
        if self.evidence.len() != required {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "membership lineage contains unrelated Abort evidence",
            ));
        }
        chain.reverse();
        let mut stopped = BTreeMap::new();
        let mut epochs = BTreeMap::new();
        for (hash, intent) in &chain {
            let node = self.node(hash)?;
            if let Some(stopped_intent) = node.stop_intent(intent) {
                let effect = stopped_intent
                    .effect
                    .as_ref()
                    .expect("invariant: verified exit effect");
                stopped.entry(effect.disk_id.clone()).or_insert((
                    stopped_intent.operation_id,
                    object_hash(INTENT_KIND, stopped_intent)?,
                    stopped_intent.origin.predecessor().map_or(
                        DiskSealFormat::ManagedDirectory,
                        |authority_hash| DiskSealFormat::MembershipGeneration {
                            authority_hash: authority_hash.into(),
                        },
                    ),
                ));
            }
            if let Some(request) = node.retained_readmission(intent)? {
                if stopped.get(&request.target.disk_id)
                    != Some(&(
                        request.stopped_operation,
                        request.stopped_transition.clone(),
                        request.original_seal.format.clone(),
                    ))
                    || epochs.get(&request.target.disk_id) != request.previous_admission.as_ref()
                {
                    return Err(Error::new(
                        ErrorCode::PublicationInvalid,
                        "readmission does not consume the exact stopped I/O lifetime",
                    ));
                }
                stopped.remove(&request.target.disk_id);
                epochs.insert(
                    request.target.disk_id.clone(),
                    object_hash(INTENT_KIND, intent)?,
                );
            }
        }
        Ok(chain)
    }

    pub(in crate::engine::membership_transition) fn stopped_readmission_source(
        &self,
        identity: &crate::DiskRegistrationIdentity,
    ) -> Result<(OperationId, String, DiskSealFormat)> {
        let (_, operation, transition) = self
            .bootstrap_stops()?
            .into_iter()
            .find(|(target, _, _)| target == identity)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::PublicationInvalid,
                    "retained registration has no current authenticated stop",
                )
            })?;
        for (hash, intent) in self.lineage()? {
            if let Some(stopped) = self.node(&hash)?.stop_intent(&intent)
                && stopped.operation_id == operation
                && object_hash(INTENT_KIND, stopped)? == transition
            {
                if stopped
                    .effect
                    .as_ref()
                    .is_none_or(|effect| effect.action != MembershipExitAction::PlannedOffline)
                {
                    return Err(Error::new(
                        ErrorCode::ConfigConflict,
                        "Detached registration cannot be readmitted",
                    ));
                }
                let format =
                    stopped
                        .origin
                        .predecessor()
                        .map_or(DiskSealFormat::ManagedDirectory, |hash| {
                            DiskSealFormat::MembershipGeneration {
                                authority_hash: hash.into(),
                            }
                        });
                return Ok((operation, transition, format));
            }
        }
        Err(Error::new(
            ErrorCode::PublicationInvalid,
            "current stop has no exact immutable source",
        ))
    }

    pub(in crate::engine::membership_transition) fn intent(&self) -> Result<TransitionIntent> {
        Ok(self
            .lineage()?
            .pop()
            .expect("invariant: lineage contains head")
            .1)
    }

    pub(in crate::engine::membership_transition) fn epoch(&self) -> Result<u64> {
        u64::try_from(self.lineage()?.len())
            .map_err(|_| Error::new(ErrorCode::ResourceLimit, "membership lineage is too large"))?
            .checked_add(1)
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimit, "membership epoch exhausted"))
    }

    pub(in crate::engine::membership_transition) fn genesis_config(
        &self,
    ) -> Result<ValidatedConfig> {
        Ok(self
            .lineage()?
            .first()
            .expect("invariant: lineage contains Genesis")
            .1
            .origin
            .config()
            .clone())
    }

    pub(super) fn contains(&self, hash: &str) -> Result<bool> {
        Ok(self.lineage()?.iter().any(|(known, _)| known == hash))
    }

    pub(super) fn matches_metadata_projection(&self, config: &ValidatedConfig) -> Result<bool> {
        for (hash, intent) in self.lineage()? {
            if config.same_metadata_membership(intent.origin.config())
                || config.same_metadata_membership(&self.node(&hash)?.effective_config(&intent)?)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(super) fn rejects_transition(&self, transition: &str) -> Result<bool> {
        for (hash, intent) in self.lineage()? {
            if matches!(self.node(&hash)?, AuthorityCore::Abort(_))
                && object_hash(INTENT_KIND, &intent)? == transition
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(super) fn verify_ancestor_candidate(&self, candidate: &Candidate) -> Result<()> {
        let chain = self.lineage()?;
        match candidate.generation() {
            Some(hash) => {
                let intent = &chain
                    .iter()
                    .find(|(known, _)| known == hash)
                    .ok_or_else(|| {
                        Error::new(
                            ErrorCode::PublicationInvalid,
                            "candidate generation is outside lineage",
                        )
                    })?
                    .1;
                let node = self.node(hash)?;
                if let AuthorityCore::Backup(core) = node {
                    return core.verify_first_slot(candidate, hash);
                }
                if let AuthorityCore::Foreign(core) = node {
                    return core.verify_first_slot(candidate, hash);
                }
                match self.registry_for_node(node)? {
                    Some(registry) => registry.verify_lifecycle_candidate(candidate, hash),
                    None => node
                        .effective_config(intent)?
                        .verify_original_candidate_membership(candidate),
                }
            }
            None => chain
                .first()
                .expect("invariant: lineage contains Genesis")
                .1
                .origin
                .config()
                .verify_original_candidate_membership(candidate),
        }
    }

    pub(super) fn frozen_archive(&self, candidate: &Candidate) -> Result<(ArchiveRoot, u64)> {
        for (archive, intent) in self.archives()? {
            if intent.origin.predecessor() == candidate.generation() {
                self.verify_ancestor_candidate(candidate)?;
                return Ok((archive, intent.fence_sequence));
            }
        }
        Err(Error::new(
            ErrorCode::PublicationInvalid,
            "candidate has no archived epoch in lineage",
        ))
    }

    pub(crate) fn verify_candidate(&self, candidate: &Candidate) -> Result<ValidatedConfig> {
        let intent = self.intent()?;
        if candidate.generation() != Some(self.hash()?.as_str())
            || candidate.sequence <= intent.fence_sequence
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "candidate does not bind this generation authority",
            ));
        }
        if let AuthorityCore::Backup(core) = &self.head {
            // Only the exact first-slot envelope is admitted here. Full State
            // replay below the generation/archive consumer is still required;
            // this cannot authorize a later slot or an active registry.
            core.verify_first_slot(candidate, &self.hash()?)?;
            return core.effective_config();
        }
        let config = self.head.effective_config(&intent)?;
        if let AuthorityCore::Foreign(core) = &self.head {
            core.verify_first_slot(candidate, &self.hash()?)?;
            return Ok(config);
        }
        match self.registry_for_node(&self.head)? {
            Some(registry) => registry.verify_lifecycle_candidate(candidate, &self.hash()?)?,
            None => config.verify_original_candidate_membership(candidate)?,
        }
        Ok(config)
    }

    fn node(&self, hash: &str) -> Result<&AuthorityCore> {
        if self.hash()? == hash {
            return Ok(&self.head);
        }
        self.predecessors.get(hash).ok_or_else(|| {
            Error::new(
                ErrorCode::ObjectCorrupt,
                "authority core is outside lineage",
            )
        })
    }

    pub(in crate::engine::membership_transition) fn evidence_entries(
        &self,
    ) -> &BTreeMap<String, AbortAuthorityEvidence> {
        &self.evidence
    }

    pub(in crate::engine::membership_transition) fn verify_detached_evidence(
        core: AuthenticatedMetadataObject,
        evidence: &AbortAuthorityEvidence,
    ) -> Result<()> {
        let core: AuthorityCore = json_codec::from_value(core.into_value())?;
        core.intent(Some(evidence))?;
        Ok(())
    }

    #[cfg(test)]
    pub(in crate::engine::membership_transition) fn abort_for_test(
        core: AbortCore,
        witness: AbortAuthorityEvidence,
        predecessor: Option<&Self>,
    ) -> Result<Self> {
        Self::new_abort(core, witness, predecessor)
    }

    pub(in crate::engine::membership_transition) fn new_abort(
        core: AbortCore,
        witness: AbortAuthorityEvidence,
        predecessor: Option<&Self>,
    ) -> Result<Self> {
        let hash = object_hash(AUTHORITY_KIND, &core)?;
        let mut predecessors = BTreeMap::new();
        let mut evidence = BTreeMap::new();
        if let Some(previous) = predecessor {
            predecessors = previous.predecessors.clone();
            predecessors.insert(previous.hash()?, previous.head.clone());
            evidence = previous.evidence.clone();
        }
        evidence.insert(hash, witness);
        let authority = Self {
            head: AuthorityCore::Abort(core),
            predecessors,
            evidence,
        };
        authority.lineage()?;
        Ok(authority)
    }
}

pub(in crate::engine::membership_transition) fn same_config_projection(
    left: &ValidatedConfig,
    right: &ValidatedConfig,
) -> bool {
    admission::same_settings(left, right)
        && left.disks.len() == right.disks.len()
        && left.disks.iter().all(|disk| {
            right.disks.iter().any(|other| {
                let mut projected = other.clone();
                projected.root.clone_from(&disk.root);
                admission::same_disk(disk, &projected)
            })
        })
}
