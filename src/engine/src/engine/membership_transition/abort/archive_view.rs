//! Read-only, complete frozen-archive admission for the abort evidence candidate.
//! No original or target device path is opened, and no archive bytes are changed.
use super::*;

pub(in crate::engine::membership_transition) struct ArchivedAbortMember {
    pub(in crate::engine::membership_transition) seal: DiskSeal,
    pub(in crate::engine::membership_transition) fence: Receipt,
}

#[cfg(test)]
pub(in crate::engine::membership_transition) struct VerifiedAbortArchive {
    root: ArchiveRoot,
    intent: TransitionIntent,
    members: BTreeMap<String, ArchivedAbortMember>,
}

#[cfg(test)]
impl VerifiedAbortArchive {
    /// The expected hashes must come from the admitted handoff, never from the
    /// directory's self-declared root. Complete F verification is unchanged.
    pub(in crate::engine::membership_transition) fn read(
        root: DiskRoot,
        expected: &MembershipControlArchive,
    ) -> Result<Self> {
        if !valid_hash(&expected.root_hash) || !valid_hash(&expected.transition_hash) {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "invalid abort archive binding",
            ));
        }
        let store = ArchiveStore { root };
        let root: ArchiveRoot = json_codec::from_value(
            AuthenticatedMetadataObject::authenticate(
                ROOT_KIND,
                &expected.root_hash,
                decode_strict(
                    &store
                        .root
                        .read_bytes(&store.path(ROOT_FILE), MAX_METADATA)?,
                )?,
            )?
            .into_value(),
        )?;
        verify_root(&store, &root, &expected.transition_hash)?;
        let intent: TransitionIntent = json_codec::from_value(
            store
                .load(INTENT_KIND, &expected.transition_hash)?
                .into_value(),
        )?;
        let entries =
            crate::metadata_map::read_map(&root.members, |kind, hash| store.load(kind, hash))?;
        let namespace = intent
            .origin
            .predecessor()
            .map(|_| expected.transition_hash.as_str());
        let mut members = BTreeMap::new();
        for disk in intent.origin.config().metadata_members() {
            let member: ArchivedMember = json_codec::from_value(entries[&disk.disk_id].clone())?;
            let bytes = store.root.read_bytes(
                &store.member_directory(disk, namespace).join("disk.json"),
                MAX_METADATA,
            )?;
            // This read follows complete F admission; authenticate it again
            // against F rather than trusting a mutable archive path forever.
            if content_hash(&bytes) != member.seal_bytes_hash {
                return Err(Error::new(
                    ErrorCode::ObjectCorrupt,
                    "abort archive seal changed after admission",
                ));
            }
            members.insert(
                disk.disk_id.clone(),
                ArchivedAbortMember {
                    seal: decode_seal(&bytes)?,
                    fence: member.fence,
                },
            );
        }
        Ok(Self {
            root,
            intent,
            members,
        })
    }

    pub(in crate::engine::membership_transition) fn root(&self) -> &ArchiveRoot {
        &self.root
    }

    pub(in crate::engine::membership_transition) fn intent(&self) -> &TransitionIntent {
        &self.intent
    }

    pub(in crate::engine::membership_transition) fn members(
        &self,
    ) -> &BTreeMap<String, ArchivedAbortMember> {
        &self.members
    }
}

/// The exact original-identity projection of F, not a claim that its journal or
/// publication closure is present. Held archive admission verifies that closure
/// independently. No config path in this projection is opened.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::engine::membership_transition) struct PortableAbortArchive {
    objects: BTreeMap<String, json_codec::Value>,
    seals: BTreeMap<String, Vec<u8>>,
}

impl PortableAbortArchive {
    pub(in crate::engine::membership_transition) fn verify(
        &self,
        root: &ArchiveRoot,
        transition: &str,
    ) -> Result<(TransitionIntent, BTreeMap<String, ArchivedAbortMember>)> {
        if root.transition_hash != transition || !valid_hash(transition) {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "abort archive transition differs",
            ));
        }
        let mut read = BTreeSet::new();
        let mut load = |kind: &'static str, hash: &str| {
            let key = format!("{kind}/{hash}");
            let value = self.objects.get(&key).ok_or_else(|| {
                Error::new(
                    ErrorCode::ObjectCorrupt,
                    "abort archive identity dependency missing",
                )
            })?;
            read.insert(key);
            AuthenticatedMetadataObject::authenticate(kind, hash, value.clone())
        };
        let intent: TransitionIntent =
            json_codec::from_value(load(INTENT_KIND, transition)?.into_value())?;
        let entries = crate::metadata_map::read_map(&root.members, &mut load)?;
        let config = intent.origin.config();
        if entries.len() != config.metadata_members().len() || self.seals.len() != entries.len() {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "abort archive original membership differs",
            ));
        }
        let mut members = BTreeMap::new();
        for disk in config.metadata_members() {
            let entry = entries.get(&disk.disk_id).ok_or_else(|| {
                Error::new(
                    ErrorCode::ObjectCorrupt,
                    "abort archive original member missing",
                )
            })?;
            let member: ArchivedMember = json_codec::from_value(entry.clone())?;
            let bytes = self.seals.get(&disk.disk_id).ok_or_else(|| {
                Error::new(
                    ErrorCode::ObjectCorrupt,
                    "abort archive original seal missing",
                )
            })?;
            if bytes.len() > MAX_METADATA || content_hash(bytes) != member.seal_bytes_hash {
                return Err(Error::new(
                    ErrorCode::ObjectCorrupt,
                    "abort archive original seal bytes differ",
                ));
            }
            let seal = decode_seal(bytes)?;
            verify_archived_member_identity(
                config,
                intent.origin.predecessor(),
                disk,
                &member,
                &seal,
            )?;
            if member.fence.disk_id != disk.disk_id
                || member.fence.slot_id != disk.slot_id
                || !receipt_binds(
                    &member.fence,
                    RecordType::MembershipFence,
                    intent.fence_sequence,
                    transition,
                    transition,
                )?
            {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "abort archive original fence differs",
                ));
            }
            members.insert(
                disk.disk_id.clone(),
                ArchivedAbortMember {
                    seal,
                    fence: member.fence,
                },
            );
        }
        if read.len() != self.objects.len() {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "abort archive identity contains unrelated objects",
            ));
        }
        Ok((intent, members))
    }

    #[cfg(test)]
    pub(in crate::engine::membership_transition) fn read_for_test(
        root: DiskRoot,
        expected: &MembershipControlArchive,
    ) -> Result<Self> {
        let admitted = VerifiedAbortArchive::read(root.private_child(root.path())?, expected)?;
        let store = ArchiveStore { root };
        let mut objects = BTreeMap::new();
        let mut load = |kind: &'static str, hash: &str| {
            let object = store.load(kind, hash)?;
            objects.insert(format!("{kind}/{hash}"), object.value().clone());
            Ok(object)
        };
        load(INTENT_KIND, &expected.transition_hash)?;
        crate::metadata_map::read_map(&admitted.root.members, &mut load)?;
        let namespace = admitted
            .intent
            .origin
            .predecessor()
            .map(|_| expected.transition_hash.as_str());
        let seals = admitted
            .intent
            .origin
            .config()
            .metadata_members()
            .map(|disk| {
                Ok((
                    disk.disk_id.clone(),
                    store.root.read_bytes(
                        &store.member_directory(disk, namespace).join("disk.json"),
                        MAX_METADATA,
                    )?,
                ))
            })
            .collect::<Result<_>>()?;
        Ok(Self { objects, seals })
    }
}
