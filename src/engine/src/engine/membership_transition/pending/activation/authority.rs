//! Flat, hash-linked generation authority. Old node bytes remain unchanged;
//! successors carry each immutable predecessor node exactly once.
use super::*;
use crate::engine::membership_transition::abort::{AbortAuthorityEvidence, AbortCore};
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
    Abort(AbortCore),
    Exit(exit::ExitCore),
}

pub(in crate::engine::membership_transition) trait AuthorityResolver {
    fn core_object(&mut self, hash: &str) -> Result<AuthenticatedMetadataObject>;
    fn abort_evidence(&mut self, hash: &str) -> Result<AbortAuthorityEvidence>;
}
impl AuthorityCore {
    fn archive(&self) -> &ArchiveRoot {
        match self {
            Self::Add(node) => &node.archive,
            Self::Abort(core) => &core.archive,
            Self::Exit(core) => &core.archive,
        }
    }
    fn intent(&self, evidence: Option<&AbortAuthorityEvidence>) -> Result<TransitionIntent> {
        match (self, evidence) {
            (Self::Add(node), None) => node.intent(),
            (Self::Exit(core), None) => core.intent(),
            (Self::Abort(core), Some(evidence)) => evidence.verify(core),
            _ => Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "authority evidence does not exactly cover Abort cores",
            )),
        }
    }
    fn effective_config<'a>(&self, intent: &'a TransitionIntent) -> &'a ValidatedConfig {
        match self {
            Self::Add(_) | Self::Exit(_) => &intent.target,
            Self::Abort(_) => intent.origin.config(),
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

impl AuthorityNode {
    fn intent(&self) -> Result<TransitionIntent> {
        let verified = self.joint.verify()?;
        if verified.intent.effect.is_some() {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "exit control evidence cannot authorize lifecycle activation",
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
        if let Some(registry) = &self.registry {
            registry.verify(&verified.intent, &verified.intent.target)?;
        }
        Ok(verified.intent)
    }
}

impl PortableAuthority {
    /// Add-only resources must never be inferred from an Abort authority.
    pub(super) fn add_joint(&self) -> Result<&PortableJoint> {
        match &self.head {
            AuthorityCore::Add(node) => Ok(&node.joint),
            AuthorityCore::Exit(core) => Ok(&core.joint),
            AuthorityCore::Abort(_) => Err(Error::new(
                ErrorCode::RecoveryRequired,
                "Abort authority has no Joint addition certificate",
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
            AuthorityCore::Add(_) | AuthorityCore::Exit(_) => Err(Error::new(
                ErrorCode::ConfigConflict,
                "expected Abort authority",
            )),
        }
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
            AuthorityCore::Add(node) => {
                if let Some(registry) = &node.registry {
                    registry.verify_source(&self.intent()?, source)?;
                }
            }
            AuthorityCore::Exit(core) => {
                core.registry_from_source(source)?;
            }
            AuthorityCore::Abort(_) => (),
        }
        Ok(())
    }

    pub(in crate::engine) fn restrict_io(
        &self,
        config: ValidatedConfig,
    ) -> Result<ValidatedConfig> {
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
        let intent = self.intent()?;
        if let AuthorityCore::Exit(core) = &self.head {
            return core.current_namespaces(source, previous);
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
    pub(in crate::engine) fn bootstrap_stops(
        &self,
    ) -> Result<Vec<(crate::DiskRegistrationIdentity, OperationId, String)>> {
        let mut stops = BTreeMap::new();
        for (hash, intent) in self.lineage()? {
            if matches!(self.node(&hash)?, AuthorityCore::Exit(_)) {
                let effect = intent
                    .effect
                    .as_ref()
                    .expect("invariant: verified exit effect");
                let binding = effect.identity(&intent.origin.config().pool_id);
                if let Some((previous, _, _)) = stops.get(&binding.disk_id) {
                    if previous != &binding {
                        return Err(Error::new(
                            ErrorCode::PublicationInvalid,
                            "stopped logical disk is rebound in authority lineage",
                        ));
                    }
                } else {
                    let transition = object_hash(INTENT_KIND, &intent)?;
                    stops.insert(
                        binding.disk_id.clone(),
                        (binding, intent.operation_id, transition),
                    );
                }
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
            .bootstrap_stops()?
            .iter()
            .any(|(identity, operation, hash)| {
                identity == binding && *operation == operation_id && hash == intent_hash
            })
        {
            Ok(())
        } else {
            Err(Error::new(
                ErrorCode::RecoveryRequired,
                "bootstrap stop has no exact first authenticated exit authority",
            ))
        }
    }

    pub(in crate::engine::membership_transition) fn is_abort(&self) -> bool {
        matches!(self.head, AuthorityCore::Abort(_))
    }
    pub(in crate::engine::membership_transition) fn effective_config(
        &self,
    ) -> Result<ValidatedConfig> {
        let intent = self.intent()?;
        Ok(self.head.effective_config(&intent).clone())
    }

    pub(super) fn workspace_root_aliases(&self) -> Result<WorkspaceRootAliases> {
        let mut roots: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
        // These names are authenticated historical locators, not permissions to
        // open an old root. Consumers must project onto the current held binding.
        for (hash, intent) in self.lineage()? {
            for config in [
                intent.origin.config(),
                self.node(&hash)?.effective_config(&intent),
            ] {
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
                Ok((node.archive().clone(), intent))
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
            head: AuthorityCore::Add(AuthorityNode {
                joint,
                archive,
                registry,
            }),
            predecessors,
            evidence,
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
            if !same_config_projection(current.origin.config(), node.effective_config(&previous))
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
                AuthorityCore::Add(node) => node
                    .registry
                    .as_ref()
                    .map(registry::RegistryContinuation::parent),
                AuthorityCore::Exit(core) => core.registry_parent(),
                AuthorityCore::Abort(_) => previous,
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
        Ok(chain)
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
                || config.same_metadata_membership(self.node(&hash)?.effective_config(&intent))
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
                match self.registry_for_node(node)? {
                    Some(registry) => registry.verify_lifecycle_candidate(candidate, hash),
                    None => node
                        .effective_config(intent)
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
        let config = self.head.effective_config(&intent);
        match self.registry_for_node(&self.head)? {
            Some(registry) => registry.verify_lifecycle_candidate(candidate, &self.hash()?)?,
            None => config.verify_original_candidate_membership(candidate)?,
        }
        Ok(config.clone())
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
