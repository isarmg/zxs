//! Read-only, complete frozen-archive admission for the abort evidence candidate.
//! No original or target device path is opened, and no archive bytes are changed.
use super::*;
use crate::persist::layout;

pub(in crate::engine::membership_transition) struct ArchivedAbortMember {
    pub(in crate::engine::membership_transition) seal: DiskSeal,
    pub(in crate::engine::membership_transition) fence: Receipt,
}

pub(in crate::engine::membership_transition) struct VerifiedAbortArchive {
    root: ArchiveRoot,
    intent: TransitionIntent,
    members: BTreeMap<String, ArchivedAbortMember>,
}

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
        if intent.fence_scope.is_some() {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "retained Exit quorum archive cannot authorize Abort",
            ));
        }
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
                &store.member_directory(disk, namespace).join(layout::DISK_IDENTITY.as_path()),
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
    #[cfg(test)]
    pub(in crate::engine::membership_transition) fn read_for_test(
        root: DiskRoot,
        expected: &MembershipControlArchive,
    ) -> Result<Self> {
        Self::read(root, expected)
    }

    /// Check the real original journal byte-for-byte against F. The active seal
    /// may advance to G, but the predecessor journal and its fence never change.
    pub(in crate::engine::membership_transition) fn verify_live_member(
        &self,
        archive: &ArchiveRoot,
        transition: &str,
        root: &DiskRoot,
        disk: &DiskConfig,
        generation: &str,
    ) -> Result<()> {
        let (intent, members) = self.verify(archive, transition)?;
        let original = members.get(&disk.disk_id).ok_or_else(|| {
            Error::new(
                ErrorCode::DiskIdentityMismatch,
                "Abort root is not an original metadata member",
            )
        })?;
        let configured = intent
            .origin
            .config()
            .disks
            .iter()
            .find(|old| old.disk_id == disk.disk_id)
            .expect("invariant: verified archive member belongs to original config");
        let mut projection = disk.clone();
        projection.root.clone_from(&configured.root);
        if !pending::admission::same_disk(configured, &projection) {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "Abort member binding differs",
            ));
        }
        let identity = pending::admission::member_identity(
            intent.origin.config(),
            configured,
            FileIdentity {
                device: original.seal.device,
                inode: original.seal.inode,
            },
        );
        identity.verify_physical_root(root)?;
        let stored = decode_seal(&root.read_entry_bytes(&layout::DISK_IDENTITY, MAX_METADATA)?)?;
        let mut expected = original.seal.clone();
        if stored.format != expected.format {
            expected.format = DiskSealFormat::MembershipGeneration {
                authority_hash: generation.to_owned(),
            };
        }
        validate_stored_seal(&stored, &expected)?;
        if matches!(&stored.format, DiskSealFormat::MembershipGeneration { authority_hash } if authority_hash == generation)
            && root
                .open_optional_read_regular(
                    &pending::activation::generation_directory(disk, generation)
                        .join(layout::JOURNAL.as_path()),
                )?
                .is_none()
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "active Abort generation journal is missing",
            ));
        }
        let entries = crate::metadata_map::read_map(&archive.members, |kind, hash| {
            let value = self.objects.get(&format!("{kind}/{hash}")).ok_or_else(|| {
                Error::new(
                    ErrorCode::ObjectCorrupt,
                    "Abort archive member page missing",
                )
            })?;
            AuthenticatedMetadataObject::authenticate(kind, hash, value.clone())
        })?;
        let member: ArchivedMember = json_codec::from_value(entries[&disk.disk_id].clone())?;
        let path = match intent.origin.predecessor() {
            Some(hash) => {
                pending::activation::generation_directory(disk, hash)
                    .join(layout::JOURNAL.as_path())
            }
            None => crate::journal::log_path(disk),
        };
        let (bytes, hash) =
            hash_original_journal(root.open_read_regular(&path)?, member.journal.bytes)?;
        if bytes != member.journal.bytes || hash != member.journal.content_hash {
            return Err(Error::new(
                ErrorCode::LogFork,
                "Abort original journal differs from exact frozen archive",
            ));
        }
        Ok(())
    }

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
        root.verify_scope(&intent)?;
        if intent.fence_scope.is_some() {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "retained Exit quorum archive cannot authorize Abort",
            ));
        }
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

    pub(in crate::engine::membership_transition) fn read(
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
                        &store
                            .member_directory(disk, namespace)
                            .join(layout::DISK_IDENTITY.as_path()),
                        MAX_METADATA,
                    )?,
                ))
            })
            .collect::<Result<_>>()?;
        Ok(Self { objects, seals })
    }
}

/// The admitted reader owns its real descriptor and checks cancellation after
/// EOF as well as during each read, before its digest can authorize a vote.
fn hash_original_journal(mut file: ReadFile, expected_bytes: u64) -> Result<(u64, String)> {
    let limit = expected_bytes.checked_add(1).ok_or_else(|| {
        Error::new(
            ErrorCode::ResourceLimit,
            "original journal byte limit overflow",
        )
    })?;
    let mut digest = blake3::Hasher::new();
    let bytes = {
        let mut reader = HashingReader {
            reader: (&mut file).take(limit),
            digest: &mut digest,
        };
        std::io::copy(&mut reader, &mut std::io::sink())?
    };
    file.check_running()?;
    Ok((bytes, digest.finalize().to_hex().to_string()))
}
