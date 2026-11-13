use super::*;
use crate::engine::membership_transition::decision::DecisionKind;

pub(super) struct PreparedProposal {
    pub(super) object: PreparedMetadataObject,
    pub(super) objects: Vec<PreparedMetadataObject>,
    pub(super) acceptances: BTreeMap<String, PreparedMetadataObject>,
    format: JointFormat,
}
impl PreparedProposal {
    pub(super) fn new(
        pending: &PendingMembership,
        fence: &AdmittedFence,
        checked: &[CheckedMember],
    ) -> Result<Self> {
        let config = fence.intent.origin.config();
        fence.intent.verify_target()?;
        if let Some(recipe) = &fence.intent.foreign {
            let (base, _) = pending.source.generation_base(&fence.intent)?;
            recipe.joint_state(
                &base,
                config,
                &fence.intent.target,
                fence.intent.fence_sequence,
            )?;
        }
        let mut encoder = Encoder::new(MAX_METADATA);
        encoder.object(INTENT_KIND, &fence.intent)?;
        let mut original = json_codec::Map::new();
        let mut targets = json_codec::Map::new();
        let mut old_only = json_codec::Map::new();
        for (member, checked) in pending.members.iter().zip(checked) {
            if let Some(receipt) = fence.receipts.get(&member.disk.disk_id) {
                original.insert(
                    member.disk.disk_id.clone(),
                    json_codec::to_value(OriginalDependency {
                        identity: member.enrollment.identity.clone(),
                        fence: receipt.clone(),
                    })?,
                );
            }
            let participants = if fence
                .intent
                .target
                .metadata_members()
                .any(|disk| disk.disk_id == member.disk.disk_id)
            {
                &mut targets
            } else {
                &mut old_only
            };
            participants.insert(
                member.disk.disk_id.clone(),
                json_codec::to_value(TargetDependency {
                    enrollment: member.enrollment.clone(),
                    bootstrap: checked.bootstrap.clone(),
                    seal: match &member.resource {
                        MemberResource::Retained(handle) => MemberSeal::Retained {
                            seal: handle.seal.clone(),
                        },
                        MemberResource::Added(handle) => MemberSeal::Added {
                            seal: handle.seal.clone(),
                        },
                    },
                    control_receipt: pending.receipts[&member.disk.disk_id].clone(),
                })?,
            );
        }
        let old_only_members = if old_only.is_empty() {
            None
        } else {
            Some(
                encoder
                    .map_root(old_only, ProofMeasure::Plain)?
                    .reference()
                    .clone(),
            )
        };
        let format = if fence.intent.fence_scope.is_some() {
            JointFormat::RetainedExitQuorum
        } else if fence.intent.effect.is_some() {
            JointFormat::ExitControl
        } else if fence.intent.foreign.is_some() {
            if pending.data_disks.is_empty() {
                JointFormat::ForeignJoint
            } else {
                JointFormat::ForeignDevices
            }
        } else if pending.data_disks.is_empty() {
            JointFormat::Joint
        } else {
            JointFormat::Devices
        };
        let data_disks = if pending.data_disks.is_empty() {
            None
        } else {
            let dependencies = pending
                .data_disks
                .iter()
                .map(|disk| {
                    Ok((
                        disk.disk.disk_id.clone(),
                        json_codec::to_value(&disk.dependency)?,
                    ))
                })
                .collect::<Result<json_codec::Map<_, _>>>()?;
            Some(
                encoder
                    .map_root(dependencies, ProofMeasure::Plain)?
                    .reference()
                    .clone(),
            )
        };
        let object = PreparedMetadataObject::new(
            PROPOSAL_KIND,
            &Proposal {
                format,
                data_disks,
                old_only_members,
                transition_hash: fence.hash.clone(),
                original_config_hash: object_hash("membership-original", config)?,
                target_config_hash: object_hash("membership-target", &fence.intent.target)?,
                control_archive_root: pending.archive_root.clone(),
                operation_id: fence.intent.operation_id,
                principal: fence.intent.principal.clone(),
                frozen_boundary: frozen_boundary(&fence.intent),
                fence_sequence: fence.intent.fence_sequence,
                original_members: encoder
                    .map_root(original, ProofMeasure::Plain)?
                    .reference()
                    .clone(),
                target_members: encoder
                    .map_root(targets, ProofMeasure::Plain)?
                    .reference()
                    .clone(),
            },
            MAX_METADATA,
        )?;
        let mut acceptances = BTreeMap::new();
        for member in &pending.members {
            let acceptance = PreparedMetadataObject::new(
                ACCEPTANCE_KIND,
                &Acceptance {
                    proposal_hash: object.hash.clone(),
                    identity: member.enrollment.identity.clone(),
                    enrollment_hash: object_hash(ENROLLMENT_KIND, &member.enrollment)?,
                    bootstrap_hash: member.bootstrap.hash.clone(),
                    control_receipt: pending.receipts[&member.disk.disk_id].clone(),
                },
                MAX_METADATA,
            )?;
            encoder.prepared(acceptance.clone());
            acceptances.insert(member.disk.disk_id.clone(), acceptance);
        }
        encoder.prepared(object.clone());
        Ok(Self {
            object,
            objects: encoder.finish(),
            acceptances,
            format,
        })
    }

    pub(super) fn certificate(
        &self,
        votes: &BTreeMap<String, JointVote>,
    ) -> Result<(PreparedMetadataObject, Vec<PreparedMetadataObject>)> {
        let mut encoder = Encoder::new(MAX_METADATA);
        let map = votes
            .iter()
            .map(|(key, vote)| Ok((key.clone(), json_codec::to_value(vote)?)))
            .collect::<Result<json_codec::Map<_, _>>>()?;
        let object = PreparedMetadataObject::new(
            CERTIFICATE_KIND,
            &Certificate {
                format: self.format,
                proposal_hash: self.object.hash.clone(),
                votes: encoder
                    .map_root(map, ProofMeasure::Plain)?
                    .reference()
                    .clone(),
            },
            MAX_METADATA,
        )?;
        encoder.prepared(object.clone());
        Ok((object, encoder.finish()))
    }
}

pub(in crate::engine::membership_transition::pending) struct JointStore {
    pub(super) root: DiskRoot,
    pub(super) decision: DecisionStore,
}
impl JointStore {
    pub(in crate::engine::membership_transition::pending) fn open(
        disk: &DiskRoot,
        parent: &DiskRoot,
        identity: &MemberIdentity,
        transition: &str,
        proposal: &str,
        install: bool,
    ) -> Result<Self> {
        let decision = DecisionStore::open(disk, identity, transition, install)?;
        let path = parent.path().join("joint");
        if install {
            parent.private_dir(&path)?;
        }
        let root = parent.private_child(&path)?;
        let names = root
            .entries(&path)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(fs_error)?;
        let binding = JointBinding {
            identity: identity.clone(),
            proposal_hash: proposal.to_owned(),
        };
        if !names.is_empty() || !install {
            let saved: JointBinding =
                decode_strict(&root.read_bytes(&path.join(BINDING_FILE), MAX_METADATA)?)?;
            if saved.identity != binding.identity || saved.proposal_hash != proposal {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "joint member already binds another proposal or physical identity",
                ));
            }
            for name in names {
                if platform_io::fs_safety::AtomicFile::is_temporary_name(&name) {
                    root.open_read_regular(&path.join(name.as_path()))?;
                } else if !matches!(
                    name.as_os_str().to_str(),
                    Some(BINDING_FILE | CERTIFICATE_FILE | "objects")
                ) {
                    return Err(Error::new(
                        ErrorCode::UnsafePath,
                        "joint directory contains unrelated material",
                    ));
                }
            }
        }
        if install {
            root.write_json(&path.join(BINDING_FILE), &binding, false)?;
        }
        verify_child(parent, &root)?;
        Ok(Self { root, decision })
    }

    pub(super) fn object_path(&self, kind: &str, hash: &str) -> Result<PathBuf> {
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || kind.is_empty()
            || !kind.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
        {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "invalid joint object address",
            ));
        }
        Ok(self
            .root
            .path()
            .join("objects")
            .join(kind)
            .join(&hash[..2])
            .join(format!("{hash}.json")))
    }
    pub(super) fn install(&self, objects: &[PreparedMetadataObject]) -> Result<()> {
        for object in objects {
            self.root.write_atomic(
                &self.object_path(object.kind(), &object.hash)?,
                object.bytes(),
                false,
            )?;
        }
        Ok(())
    }
    pub(super) fn load(
        &self,
        kind: &'static str,
        hash: &str,
    ) -> Result<AuthenticatedMetadataObject> {
        AuthenticatedMetadataObject::authenticate(
            kind,
            hash,
            decode_strict(
                &self
                    .root
                    .read_bytes(&self.object_path(kind, hash)?, MAX_METADATA)?,
            )?,
        )
    }
    #[cfg(test)]
    pub(super) fn typed<T: for<'de> Deserialize<'de>>(
        &self,
        kind: &'static str,
        hash: &str,
    ) -> Result<T> {
        Ok(json_codec::from_value(self.load(kind, hash)?.into_value())?)
    }

    pub(super) fn verify_proposal(&self, expected: &str) -> Result<VerifiedProposal> {
        verify_proposal_with(&mut |kind, hash| self.load(kind, hash), expected)
    }
    pub(in crate::engine::membership_transition::pending) fn verify_certificate(
        &self,
        expected: &str,
        proposal_hash: &str,
        local: &DiskConfig,
    ) -> Result<()> {
        self.verify_local_certificate(expected, proposal_hash, local, true)
    }
    /// Discovery verifies only existing bytes; it never creates or synchronizes
    /// a decision and cannot manufacture a durable receipt from a cached status.
    pub(in crate::engine::membership_transition::pending) fn verify_certificate_read_only(
        &self,
        expected: &str,
        proposal_hash: &str,
        local: &DiskConfig,
    ) -> Result<()> {
        self.verify_local_certificate(expected, proposal_hash, local, false)
    }
    fn verify_local_certificate(
        &self,
        expected: &str,
        proposal_hash: &str,
        local: &DiskConfig,
        synchronize: bool,
    ) -> Result<()> {
        let (_, votes) = verify_certificate_with(
            &mut |kind, hash| self.load(kind, hash),
            expected,
            proposal_hash,
        )?;
        let vote = votes.get(&local.disk_id).ok_or_else(missing_member)?;
        if vote.identity.disk_id != local.disk_id
            || vote.identity.slot_id != local.slot_id
            || vote.identity.binding_generation != local.binding_generation
        {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "certificate local member differs",
            ));
        }
        self.decision.verify_receipt(&vote.decision, synchronize)
    }
    pub(in crate::engine::membership_transition::pending) fn portable(
        &self,
        certificate_hash: &str,
        proposal_hash: &str,
    ) -> Result<PortableJoint> {
        let mut objects = BTreeMap::new();
        verify_certificate_with(
            &mut |kind, hash| {
                let object = self.load(kind, hash)?;
                objects.insert(format!("{kind}/{hash}"), object.value().clone());
                Ok(object)
            },
            certificate_hash,
            proposal_hash,
        )?;
        Ok(PortableJoint {
            certificate_hash: certificate_hash.to_owned(),
            proposal_hash: proposal_hash.to_owned(),
            objects,
        })
    }

    pub(super) fn synchronize_certificate(
        &self,
        certificate: &PreparedMetadataObject,
        install: bool,
        #[cfg(feature = "fault-injection")] fault: Option<OpeningFault>,
    ) -> Result<()> {
        let path = self.root.path().join(CERTIFICATE_FILE);
        if install {
            self.root.write_atomic(&path, certificate.bytes(), false)?;
        }
        let file = self.root.open_read_regular(&path)?;
        let bytes = read_file_bytes(file.try_clone()?, MAX_METADATA)?;
        AuthenticatedMetadataObject::authenticate(
            CERTIFICATE_KIND,
            &certificate.hash,
            decode_strict(&bytes)?,
        )?;
        // Confirmation fence is also required for an already-visible completion
        // root. Injected failures here are not simulated physical power loss.
        #[cfg(feature = "fault-injection")]
        if fault == Some(OpeningFault::FileSync) {
            return Err(std::io::Error::from_raw_os_error(5).into());
        }
        file.sync_all()?;
        #[cfg(feature = "fault-injection")]
        if fault == Some(OpeningFault::DirectorySync) {
            return Err(std::io::Error::from_raw_os_error(5).into());
        }
        self.root.sync_dir(self.root.path())
    }
}

type ObjectReader<'a> = dyn FnMut(&'static str, &str) -> Result<AuthenticatedMetadataObject> + 'a;
fn typed<T: for<'de> Deserialize<'de>>(
    load: &mut ObjectReader<'_>,
    kind: &'static str,
    hash: &str,
) -> Result<T> {
    Ok(json_codec::from_value(load(kind, hash)?.into_value())?)
}
fn verify_proposal_with(load: &mut ObjectReader<'_>, expected: &str) -> Result<VerifiedProposal> {
    let proposal: Proposal = typed(load, PROPOSAL_KIND, expected)?;
    let intent: TransitionIntent = typed(load, INTENT_KIND, &proposal.transition_hash)?;
    reservation::verify_intent_roots(&intent)?;
    let config = intent.origin.config();
    intent.verify_target()?;
    if matches!(
        proposal.format,
        JointFormat::ForeignJoint | JointFormat::ForeignDevices
    ) != intent.foreign.is_some()
        || matches!(
            proposal.format,
            JointFormat::ExitControl | JointFormat::RetainedExitQuorum
        ) != intent.effect.is_some()
        || (proposal.format == JointFormat::RetainedExitQuorum) != intent.fence_scope.is_some()
    {
        return Err(Error::new(
            ErrorCode::ConfigConflict,
            "joint effect and certificate purpose differ",
        ));
    }
    if proposal.original_config_hash != object_hash("membership-original", config)?
        || proposal.target_config_hash != object_hash("membership-target", &intent.target)?
        || proposal.operation_id != intent.operation_id
        || proposal.principal != intent.principal
        || proposal.fence_sequence != intent.fence_sequence
        || proposal.frozen_boundary != frozen_boundary(&intent)
    {
        return Err(Error::new(
            ErrorCode::LogFork,
            "joint proposal differs from original transition",
        ));
    }
    let originals =
        crate::metadata_map::read_map(&proposal.original_members, |kind, hash| load(kind, hash))?;
    let targets =
        crate::metadata_map::read_map(&proposal.target_members, |kind, hash| load(kind, hash))?;
    let original_members = intent.fence_members();
    if originals.len() != original_members.len()
        || targets.len() != intent.target.metadata_members().len()
    {
        return Err(Error::new(
            ErrorCode::ConfigConflict,
            "joint proposal member sets differ",
        ));
    }
    let old_only = match &proposal.old_only_members {
        Some(reference) if intent.effect.is_some() => {
            crate::metadata_map::read_map(reference, |kind, hash| load(kind, hash))?
        }
        None => json_codec::Map::new(),
        Some(_) => {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "pure additions cannot include old-only control participants",
            ));
        }
    };
    let old_only_count = if intent.fence_scope.is_some() {
        0
    } else {
        intent.old_only().count()
    };
    if old_only.len() != old_only_count
        || (old_only.is_empty() && proposal.old_only_members.is_some())
    {
        return Err(missing_member());
    }
    let mut dependencies = BTreeMap::new();
    let mut physical = BTreeSet::new();
    for disk in &intent.participants() {
        let selected = if intent
            .target
            .metadata_members()
            .any(|target| target.disk_id == disk.disk_id)
        {
            &targets
        } else {
            &old_only
        };
        let dependency: TargetDependency = json_codec::from_value(
            selected
                .get(&disk.disk_id)
                .ok_or_else(missing_member)?
                .clone(),
        )?;
        verify_target(&dependency, disk, &proposal, &intent)?;
        let identity = &dependency.enrollment.identity;
        if !physical.insert((identity.device, identity.inode)) {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "joint targets repeat physical identity",
            ));
        }
        if let Some(original) = originals.get(&disk.disk_id) {
            let original: OriginalDependency = json_codec::from_value(original.clone())?;
            let retained = config
                .metadata_members()
                .find(|old| old.disk_id == disk.disk_id)
                .ok_or_else(missing_member)?;
            if original.identity != *identity
                || original.identity.slot_id != retained.slot_id
                || original.identity.binding_generation != retained.binding_generation
                || original.fence.disk_id != retained.disk_id
                || original.fence.slot_id != retained.slot_id
                || !receipt_binds(
                    &original.fence,
                    RecordType::MembershipFence,
                    intent.fence_sequence,
                    &proposal.transition_hash,
                    &proposal.transition_hash,
                )?
                || !matches!(
                    dependency.enrollment.origin,
                    EnrollmentOrigin::Retained { .. }
                )
            {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "joint original fence identity or receipt differs",
                ));
            }
        } else if !matches!(dependency.enrollment.origin, EnrollmentOrigin::Added { .. }) {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "joint added member has retained origin",
            ));
        }
        dependencies.insert(disk.disk_id.clone(), dependency);
    }
    for disk in &original_members {
        if !originals.contains_key(&disk.disk_id) || !dependencies.contains_key(&disk.disk_id) {
            return Err(missing_member());
        }
    }
    let added_data = intent
        .target
        .disks
        .iter()
        .filter(|disk| {
            !disk.metadata_member && !config.disks.iter().any(|old| old.disk_id == disk.disk_id)
        })
        .collect::<Vec<_>>();
    let data_values = match (&proposal.format, &proposal.data_disks) {
        (
            JointFormat::Joint
            | JointFormat::ForeignJoint
            | JointFormat::ExitControl
            | JointFormat::RetainedExitQuorum,
            None,
        ) if added_data.is_empty() => json_codec::Map::new(),
        (JointFormat::Devices | JointFormat::ForeignDevices, Some(reference))
            if !added_data.is_empty() =>
        {
            crate::metadata_map::read_map(reference, |kind, hash| load(kind, hash))?
        }
        _ => {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "joint device dependency format differs",
            ));
        }
    };
    if data_values.len() != added_data.len() {
        return Err(Error::new(
            ErrorCode::ConfigConflict,
            "joint added data device set differs",
        ));
    }
    let mut data_disks = BTreeMap::new();
    for disk in added_data {
        let dependency: data::DataDependency = json_codec::from_value(
            data_values
                .get(&disk.disk_id)
                .ok_or_else(missing_member)?
                .clone(),
        )?;
        dependency.verify(disk, &intent, &proposal.control_archive_root)?;
        let identity = &dependency.seal.identity;
        if !physical.insert((identity.device, identity.inode)) {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "joint data device repeats physical identity",
            ));
        }
        data_disks.insert(disk.disk_id.clone(), dependency);
    }
    Ok(VerifiedProposal {
        data_disks,
        proposal,
        intent,
        dependencies,
    })
}
fn verify_certificate_with(
    load: &mut ObjectReader<'_>,
    expected: &str,
    proposal_hash: &str,
) -> Result<(VerifiedProposal, BTreeMap<String, JointVote>)> {
    let certificate: Certificate = typed(load, CERTIFICATE_KIND, expected)?;
    if certificate.proposal_hash != proposal_hash {
        return Err(Error::new(
            ErrorCode::LogFork,
            "joint certificate selects another proposal",
        ));
    }
    let verified = verify_proposal_with(load, proposal_hash)?;
    if certificate.format != verified.proposal.format {
        return Err(Error::new(
            ErrorCode::LogFork,
            "certificate format differs from proposal",
        ));
    }
    let votes = crate::metadata_map::read_map(&certificate.votes, |kind, hash| load(kind, hash))?;
    if verified.intent.effect.is_none() && votes.len() != verified.dependencies.len() {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "joint certificate requires every target vote",
        ));
    }
    let config = verified.intent.origin.config();
    let mut checked_votes = BTreeMap::new();
    let mut old_votes = 0;
    let mut target_votes = 0;
    for (disk_id, value) in &votes {
        let vote: JointVote = json_codec::from_value(value.clone())?;
        let dependency = verified
            .dependencies
            .get(disk_id)
            .ok_or_else(missing_member)?;
        let acceptance: Acceptance = typed(load, ACCEPTANCE_KIND, &vote.acceptance_hash)?;
        if vote.identity != dependency.enrollment.identity
            || acceptance.proposal_hash != proposal_hash
            || acceptance.identity != vote.identity
            || acceptance.enrollment_hash != object_hash(ENROLLMENT_KIND, &dependency.enrollment)?
            || acceptance.bootstrap_hash != object_hash(BOOTSTRAP_KIND, &dependency.bootstrap)?
            || acceptance.control_receipt != dependency.control_receipt
        {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "joint vote member binding differs",
            ));
        }
        vote.decision.verify(
            DecisionKind::Joint,
            &verified.proposal.transition_hash,
            &vote.identity,
            proposal_hash,
            &vote.acceptance_hash,
        )?;
        if config.metadata_members().any(|old| {
            old.disk_id == vote.identity.disk_id
                && old.slot_id == vote.identity.slot_id
                && old.binding_generation == vote.identity.binding_generation
        }) {
            old_votes += 1;
        }
        if verified.intent.target.metadata_members().any(|target| {
            target.disk_id == vote.identity.disk_id
                && target.slot_id == vote.identity.slot_id
                && target.binding_generation == vote.identity.binding_generation
        }) {
            target_votes += 1;
        }
        checked_votes.insert(disk_id.clone(), vote);
    }
    if old_votes < config.metadata_quorum()
        || target_votes < verified.intent.target.metadata_quorum()
    {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "joint certificate lacks independent old and target quorums",
        ));
    }
    Ok((verified, checked_votes))
}

/// Portable authority contains only membership evidence, never user State or bodies.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::engine::membership_transition) struct PortableJoint {
    pub(in crate::engine::membership_transition) certificate_hash: String,
    pub(in crate::engine::membership_transition) proposal_hash: String,
    objects: BTreeMap<String, json_codec::Value>,
}
impl PortableJoint {
    pub(in crate::engine::membership_transition) fn verify(&self) -> Result<VerifiedProposal> {
        let mut read = BTreeSet::new();
        let (verified, _) = verify_certificate_with(
            &mut |kind, hash| {
                let key = format!("{kind}/{hash}");
                let value = self.objects.get(&key).ok_or_else(|| {
                    Error::new(
                        ErrorCode::ObjectCorrupt,
                        "portable membership dependency missing",
                    )
                })?;
                read.insert(key);
                AuthenticatedMetadataObject::authenticate(kind, hash, value.clone())
            },
            &self.certificate_hash,
            &self.proposal_hash,
        )?;
        if read.len() != self.objects.len() {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "portable membership contains unrelated objects",
            ));
        }
        Ok(verified)
    }
}
pub(in crate::engine::membership_transition) struct VerifiedProposal {
    pub(in crate::engine::membership_transition::pending) proposal: Proposal,
    pub(in crate::engine::membership_transition) intent: TransitionIntent,
    pub(in crate::engine::membership_transition::pending) dependencies:
        BTreeMap<String, TargetDependency>,
    pub(in crate::engine::membership_transition::pending) data_disks:
        BTreeMap<String, data::DataDependency>,
}
fn missing_member() -> Error {
    Error::new(
        ErrorCode::ConfigConflict,
        "joint exact member set is incomplete",
    )
}
fn frozen_boundary(intent: &TransitionIntent) -> FrozenBoundary {
    match &intent.boundary {
        None => FrozenBoundary::EmptyGenesis,
        Some(boundary) => FrozenBoundary::Published {
            boundary: boundary.clone(),
        },
    }
}
fn verify_target(
    dependency: &TargetDependency,
    disk: &DiskConfig,
    proposal: &Proposal,
    intent: &TransitionIntent,
) -> Result<()> {
    let enrollment = &dependency.enrollment;
    let bootstrap = &dependency.bootstrap;
    let identity = &enrollment.identity;
    if identity.pool_id != intent.target.pool_id
        || identity.disk_id != disk.disk_id
        || identity.slot_id != disk.slot_id
        || identity.binding_generation != disk.binding_generation
        || identity.simulation != (intent.target.mode == Mode::Development)
        || enrollment.transition_hash != proposal.transition_hash
        || enrollment.target_config_hash != proposal.target_config_hash
        || enrollment.control_archive_root != proposal.control_archive_root
        || bootstrap.identity != *identity
        || bootstrap.enrollment_hash != object_hash(ENROLLMENT_KIND, enrollment)?
        || bootstrap.transition_hash != proposal.transition_hash
        || bootstrap.target_config_hash != proposal.target_config_hash
        || bootstrap.control_archive_root != proposal.control_archive_root
        || bootstrap.fence_sequence != proposal.fence_sequence
        || bootstrap.frozen_boundary != proposal.frozen_boundary
    {
        return Err(Error::new(
            ErrorCode::DiskIdentityMismatch,
            "joint target control dependency differs",
        ));
    }
    match (&enrollment.origin, &dependency.seal) {
        (EnrollmentOrigin::Retained { original_seal_hash }, MemberSeal::Retained { seal }) => {
            if let Origin::UnifiedGenesis { evidence, .. } = &intent.origin {
                evidence.require_original_seal(seal)?;
            }
            let expected_format = intent.origin.retained_seal_format()?;
            if object_hash("membership-original-seal", seal)? != *original_seal_hash
                || seal.format != expected_format
                || seal.pool_id != identity.pool_id
                || seal.disk_id != identity.disk_id
                || seal.slot_id != identity.slot_id
                || seal.binding_generation != identity.binding_generation
                || seal.device != identity.device
                || seal.inode != identity.inode
                || seal.simulation != identity.simulation
            {
                return Err(Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "joint original seal differs",
                ));
            }
        }
        (EnrollmentOrigin::Added { pending_seal_hash }, MemberSeal::Added { seal }) => {
            if intent.added_roots.get(&disk.disk_id) != Some(identity)
                || object_hash(PENDING_SEAL_KIND, seal)? != *pending_seal_hash
                || seal.identity != *identity
                || seal.transition_hash != proposal.transition_hash
                || seal.target_config_hash != proposal.target_config_hash
                || seal.control_archive_root != proposal.control_archive_root
            {
                return Err(Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "joint pending seal differs",
                ));
            }
        }
        _ => {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "joint seal origin differs",
            ));
        }
    }
    let hash = object_hash(BOOTSTRAP_KIND, bootstrap)?;
    verify_singleton(
        &dependency.control_receipt,
        identity,
        RecordType::MembershipControlBootstrap,
        proposal.fence_sequence,
        &hash,
        &hash,
    )
}
fn verify_singleton(
    receipt: &Receipt,
    identity: &MemberIdentity,
    kind: RecordType,
    sequence: u64,
    candidate: &str,
    object: &str,
) -> Result<()> {
    if receipt.disk_id != identity.disk_id
        || receipt.slot_id != identity.slot_id
        || receipt.record.previous_frame_hash != "0".repeat(64)
        || !receipt_binds(receipt, kind, sequence, candidate, object)?
    {
        return Err(Error::new(
            ErrorCode::LogFork,
            "joint dependency receipt differs from exact member",
        ));
    }
    Ok(())
}
