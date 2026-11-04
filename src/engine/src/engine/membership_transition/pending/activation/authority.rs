//! Flat, hash-linked generation authority. Old node bytes remain unchanged;
//! successors carry each immutable predecessor node exactly once.
use super::*;
use std::{collections::BTreeSet, ops::Deref};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct AuthorityNode {
    pub(super) joint: PortableJoint,
    pub(super) archive: ArchiveRoot,
}

#[derive(Clone, Debug)]
pub(crate) struct PortableAuthority {
    head: AuthorityNode,
    predecessors: BTreeMap<String, AuthorityNode>,
}

#[derive(Serialize, Deserialize)]
enum ChainFormat {
    #[serialization(rename = "membership-generation-chain")]
    MembershipGenerationChain,
}

#[derive(Serialize)]
struct ChainRef<'a> {
    format: ChainFormat,
    head: &'a AuthorityNode,
    predecessors: &'a BTreeMap<String, AuthorityNode>,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
struct Chain {
    format: ChainFormat,
    head: AuthorityNode,
    predecessors: BTreeMap<String, AuthorityNode>,
}

#[derive(Deserialize)]
#[serialization(untagged)]
enum StoredAuthority {
    Chain(Chain),
    Genesis(AuthorityNode),
}

impl Serialize for PortableAuthority {
    fn serialize<S: data_serialization::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        if self.predecessors.is_empty() {
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
        Ok(match StoredAuthority::deserialize(deserializer)? {
            StoredAuthority::Genesis(head) => Self {
                head,
                predecessors: BTreeMap::new(),
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
                Self { head, predecessors }
            }
        })
    }
}

impl Deref for PortableAuthority {
    type Target = AuthorityNode;
    fn deref(&self) -> &Self::Target {
        &self.head
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
        Ok(verified.intent)
    }
}

impl PortableAuthority {
    pub(super) fn workspace_root_aliases(&self) -> Result<WorkspaceRootAliases> {
        let mut roots: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
        // These names are authenticated historical locators, not permissions to
        // open an old root. Consumers must project onto the current held binding.
        for (_, intent) in self.lineage()? {
            for config in [intent.origin.config(), &intent.target] {
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
            let mut verified = node.joint.verify()?;
            if let Some(dependency) = verified.data_disks.remove(disk_id) {
                return Ok(Some(dependency));
            }
        }
        Ok(None)
    }
    pub(in crate::engine::membership_transition) fn load(
        hash: &str,
        mut load: impl FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject>,
    ) -> Result<Self> {
        let head: AuthorityNode = json_codec::from_value(load(AUTHORITY_KIND, hash)?.into_value())?;
        let mut predecessors = BTreeMap::new();
        let mut next = head.intent()?.origin.predecessor().map(str::to_owned);
        while let Some(hash) = next {
            if predecessors.contains_key(&hash) {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "cyclic archived membership lineage",
                ));
            }
            let node: AuthorityNode =
                json_codec::from_value(load(AUTHORITY_KIND, &hash)?.into_value())?;
            next = node.intent()?.origin.predecessor().map(str::to_owned);
            predecessors.insert(hash, node);
        }
        let authority = Self { head, predecessors };
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
                Ok((node.archive.clone(), intent))
            })
            .collect()
    }

    pub(super) fn new(
        joint: PortableJoint,
        archive: ArchiveRoot,
        predecessor: Option<&Self>,
    ) -> Result<Self> {
        let mut predecessors = BTreeMap::new();
        if let Some(previous) = predecessor {
            predecessors = previous.predecessors.clone();
            predecessors.insert(previous.hash()?, previous.head.clone());
        }
        let authority = Self {
            head: AuthorityNode { joint, archive },
            predecessors,
        };
        authority.lineage()?;
        Ok(authority)
    }

    pub(crate) fn hash(&self) -> Result<String> {
        Ok(object_hash(AUTHORITY_KIND, &self.head)?)
    }

    pub(super) fn archive_hash(&self) -> Result<String> {
        Ok(object_hash(
            "membership-control-archive",
            &self.head.archive,
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
            let intent = node.intent()?;
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
            let previous = node.intent()?;
            let current = &chain
                .last()
                .expect("invariant: current authority appended")
                .1;
            if !same_config_projection(current.origin.config(), &previous.target)
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
        if visited.len() != self.predecessors.len() + 1 {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "membership lineage contains unrelated nodes",
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

    pub(super) fn epoch(&self) -> Result<u64> {
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

    pub(super) fn verify_ancestor_candidate(&self, candidate: &Candidate) -> Result<()> {
        let chain = self.lineage()?;
        let config = match candidate.generation() {
            Some(hash) => {
                &chain
                    .iter()
                    .find(|(known, _)| known == hash)
                    .ok_or_else(|| {
                        Error::new(
                            ErrorCode::PublicationInvalid,
                            "candidate generation is outside lineage",
                        )
                    })?
                    .1
                    .target
            }
            None => chain
                .first()
                .expect("invariant: lineage contains Genesis")
                .1
                .origin
                .config(),
        };
        config.verify_original_candidate_membership(candidate)
    }

    pub(super) fn frozen_archive(&self, candidate: &Candidate) -> Result<(ArchiveRoot, u64)> {
        for (archive, intent) in self.archives()? {
            if intent.origin.predecessor() == candidate.generation() {
                intent
                    .origin
                    .config()
                    .verify_original_candidate_membership(candidate)?;
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
        intent
            .target
            .verify_original_candidate_membership(candidate)?;
        Ok(intent.target)
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
