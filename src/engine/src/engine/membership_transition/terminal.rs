//! The old fixed membership certifies an exact management handoff. Control
//! history stays independently verifiable after every metadata root is closed.
use super::archive::persistent::PendingControlArchive;
use super::pending::activation::PortableAuthority;
use super::*;
use crate::engine::disk_registry::published::{
    continuation::RegistryBoundary,
    occurrence::{FrozenDetachSource, ProjectionError},
    terminal::TerminalRegistry,
};
use crate::engine::disk_registry::{
    DiskRegistryCommitment, RegistryExitEffect, RegistryExitRequest, VerifiedDiskRegistry,
};
use std::{path::Path, sync::Arc};

pub(in crate::engine) const AUTHORITY_KIND: &str = "terminal-management-authority";

#[derive(Clone, Debug, Serialize, Deserialize)]
enum TerminalFormat {
    #[serialization(rename = "certified-terminal-management")]
    Management,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct GraphObject {
    kind: String,
    value: json_codec::Value,
}

/// A bounded immutable control object. Its hash must be selected by the held
/// protected system ledger, not supplied as an untrusted directory assertion.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::engine) struct TerminalAuthority {
    format: TerminalFormat,
    intent: TransitionIntent,
    archive_root: String,
    predecessor: Option<PortableAuthority>,
    source_registry: DiskRegistryCommitment,
    registry: DiskRegistryCommitment,
    objects: BTreeMap<String, GraphObject>,
}

pub(in crate::engine) struct VerifiedTerminalAuthority {
    pub(in crate::engine) authority: TerminalAuthority,
    pub(in crate::engine) registry: TerminalRegistry,
    pub(in crate::engine) state: Arc<State>,
    pub(in crate::engine) history: BTreeMap<u64, String>,
    pub(in crate::engine) namespaces:
        crate::engine::disk_registry::current::PreparedCurrentNamespaces,
}

impl TerminalAuthority {
    pub(in crate::engine) fn hash(&self) -> Result<String> {
        Ok(object_hash(AUTHORITY_KIND, self)?)
    }
    pub(in crate::engine) fn transition_hash(&self) -> Result<String> {
        Ok(object_hash(INTENT_KIND, &self.intent)?)
    }
    pub(in crate::engine) fn operation_id(&self) -> OperationId {
        self.intent.operation_id
    }
    pub(in crate::engine) fn target(&self) -> crate::DiskRegistrationIdentity {
        self.intent
            .effect
            .as_ref()
            .expect("invariant: terminal authority has exit")
            .identity(&self.intent.origin.config().pool_id)
    }
    pub(in crate::engine) fn action(&self) -> MembershipExitAction {
        self.intent
            .effect
            .as_ref()
            .expect("invariant: terminal authority has exit")
            .action
    }
    pub(in crate::engine) fn original(&self) -> &ValidatedConfig {
        self.intent.origin.config()
    }
    pub(in crate::engine) fn io_admission_epoch(&self, disk_id: &str) -> Result<Option<String>> {
        self.predecessor
            .as_ref()
            .map(|authority| authority.io_admission_epoch(disk_id))
            .transpose()
            .map(Option::flatten)
    }
    pub(in crate::engine) fn management_state(
        &self,
        frozen: &State,
    ) -> Result<(Arc<State>, OperationResult)> {
        let result = self.management_result(frozen)?;
        let mut state = frozen.clone();
        state.sequence = result.sequence;
        // management_result checked this same frozen generation for overflow.
        state.topology_generation += 1;
        state.operations.insert(result.operation_id, result.clone());
        state
            .operation_principals
            .insert(result.operation_id, self.intent.principal.clone());
        Ok((Arc::new(state), result))
    }

    /// Prescribed permanent receipt without cloning the frozen business state.
    pub(in crate::engine::membership_transition) fn management_result(
        &self,
        frozen: &State,
    ) -> Result<OperationResult> {
        if frozen.operations.contains_key(&self.operation_id()) {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "terminal exit operation already belongs to its frozen history",
            ));
        }
        let sequence =
            self.intent.fence_sequence.checked_add(1).ok_or_else(|| {
                Error::new(ErrorCode::ResourceLimit, "terminal sequence exhausted")
            })?;
        let topology_generation = frozen.topology_generation.checked_add(1).ok_or_else(|| {
            Error::new(
                ErrorCode::ResourceLimit,
                "terminal topology generation exhausted",
            )
        })?;
        let target = self.target();
        Ok(OperationResult {
            operation_id: self.operation_id(),
            kind: match self.action() {
                MembershipExitAction::PlannedOffline => "disk-planned-offline",
                MembershipExitAction::Deregister => "disk-deregister",
            }
            .into(),
            commit_id: None,
            history_tree_id: None,
            branch_id: None,
            commit_state: storage_contract::CommitState::Published,
            protection_at_publish: storage_contract::ProtectionAtPublish::NotApplicable,
            // No content probe occurs in management. Published certifies this
            // control operation, not live content availability or write health.
            current_health: storage_contract::CurrentHealth::Unknown,
            request_digest: self.transition_hash()?,
            sequence,
            result: json_codec::json!({
                "authority": self.hash()?, "authority_kind": "terminal-management",
                "disk_id": target.disk_id, "slot_id": target.slot_id,
                "binding_generation": target.binding_generation.to_string(),
                "registration": match self.action() { MembershipExitAction::PlannedOffline => "disabled", MembershipExitAction::Deregister => "detached" },
                "target_io": "closed", "active_metadata_members": 0,
                "topology_generation": topology_generation.to_string(),
            }),
        })
    }
    pub(in crate::engine) fn archive(&self, system: &Path) -> MembershipControlArchive {
        MembershipControlArchive {
            archive_path: system
                .join("membership-control-archives")
                .join(self.operation_id().to_string()),
            root_hash: self.archive_root.clone(),
            transition_hash: self
                .transition_hash()
                .expect("invariant: admitted terminal intent serializes"),
        }
    }

    fn read(&self, kind: &'static str, hash: &str) -> Result<Vec<u8>> {
        let object = self.objects.get(hash).ok_or_else(|| {
            Error::new(
                ErrorCode::ObjectCorrupt,
                "terminal registry graph object is missing",
            )
        })?;
        if object.kind != kind {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "terminal registry object kind differs",
            ));
        }
        let prepared = PreparedMetadataObject::new(kind, &object.value, MAX_METADATA)?;
        if prepared.hash != hash {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "terminal registry object hash differs",
            ));
        }
        Ok(prepared.bytes().to_vec())
    }

    pub(in crate::engine::membership_transition) fn validate_intent(&self) -> Result<()> {
        let effect = self.intent.effect.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::PublicationInvalid,
                "terminal authority lacks exit intent",
            )
        })?;
        effect.validate(self.original(), &self.intent.target)?;
        if !effect.is_terminal()
            || self.intent.boundary.is_none()
            || self.intent.fence_scope.is_some()
            || !self.intent.added_roots.is_empty()
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "terminal authority is not the exact final-voter fence",
            ));
        }
        match (&self.predecessor, self.intent.origin.predecessor()) {
            (None, None) => Ok(()),
            (Some(predecessor), Some(hash)) if predecessor.hash()? == hash => {
                if !pending::activation::authority_config_matches(
                    self.original(),
                    &predecessor.effective_config()?,
                ) {
                    return Err(Error::new(
                        ErrorCode::LogFork,
                        "terminal original configuration differs from its predecessor",
                    ));
                }
                Ok(())
            }
            _ => Err(Error::new(
                ErrorCode::LogFork,
                "terminal authority breaks its exact predecessor chain",
            )),
        }
    }

    fn source_registry(&self, state: &State) -> Result<VerifiedDiskRegistry> {
        self.source_registry_at(state.topology_generation)
    }
    pub(in crate::engine::membership_transition) fn source_registry_at(
        &self,
        topology_generation: u64,
    ) -> Result<VerifiedDiskRegistry> {
        let boundary = self
            .intent
            .boundary
            .as_ref()
            .expect("invariant: verified terminal boundary");
        match self
            .predecessor
            .as_ref()
            .map(PortableAuthority::verified_registry)
            .transpose()?
            .flatten()
        {
            Some(previous) => previous.verify_continuation(
                self.original(),
                self.original(),
                RegistryBoundary {
                    authority: self
                        .intent
                        .origin
                        .predecessor()
                        .expect("invariant: predecessor registry has authority"),
                    snapshot: &boundary.snapshot_hash,
                    sequence: boundary.sequence,
                    topology_generation,
                },
                &self.source_registry,
                |kind, hash| self.read(kind, hash),
            ),
            None => VerifiedDiskRegistry::verify_frozen_source(
                self.original(),
                self.intent.origin.predecessor(),
                &boundary.snapshot_hash,
                boundary.sequence,
                topology_generation,
                &self.source_registry,
                |kind, hash| self.read(kind, hash),
            ),
        }
    }

    pub(in crate::engine::membership_transition) fn intent(&self) -> &TransitionIntent {
        &self.intent
    }
    pub(in crate::engine) fn predecessor(&self) -> Option<&PortableAuthority> {
        self.predecessor.as_ref()
    }
    pub(in crate::engine) fn registry_commitment(&self) -> &DiskRegistryCommitment {
        &self.registry
    }
    pub(in crate::engine) fn archive_root_hash(&self) -> &str {
        &self.archive_root
    }
    pub(in crate::engine) fn read_graph_object(
        &self,
        kind: &'static str,
        hash: &str,
    ) -> Result<Vec<u8>> {
        self.read(kind, hash)
    }

    /// Reads only the reserved system control archive. Every historical journal,
    /// publication, permanent receipt and predecessor is independently checked.
    pub(in crate::engine) fn verify_archive(
        self,
        system: &DiskRoot,
    ) -> Result<VerifiedTerminalAuthority> {
        self.validate_intent()?;
        let expected = self.archive(system.path());
        let archive = PendingControlArchive::from_installed(
            system.private_child(&expected.archive_path)?,
            &expected.root_hash,
            &expected.transition_hash,
        )?;
        self.verify_installed_archive(&archive)
    }

    pub(in crate::engine::membership_transition) fn verify_installed_archive(
        self,
        archive: &PendingControlArchive,
    ) -> Result<VerifiedTerminalAuthority> {
        self.validate_intent()?;
        if object_hash("membership-control-archive", &archive.generation_root())?
            != self.archive_root
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "terminal installed archive root differs",
            ));
        }
        let (state, history) = archive.generation_base(&self.intent)?;
        let state = Arc::new(state);
        let source = self.source_registry(&state)?;
        let registry =
            source.verify_terminal_successor(&self.registry, |kind, hash| self.read(kind, hash))?;
        let request = registry.exit();
        let transition_hash = self.transition_hash()?;
        if request.target != self.target()
            || request.operation_id != self.operation_id()
            || request.principal != self.intent.principal
            || request.intent_hash != transition_hash
            || request.request_digest != transition_hash
            || !matches!(
                (&request.effect, self.action()),
                (
                    RegistryExitEffect::Disable,
                    MembershipExitAction::PlannedOffline
                ) | (
                    RegistryExitEffect::Detach { .. },
                    MembershipExitAction::Deregister
                )
            )
        {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "terminal registry differs from its certified exit intent",
            ));
        }
        let mut replay = pending::activation::exit::replay::NamespaceReplay::new();
        archive.replay_publications(|publication| replay.advance(publication))?;
        let boundary = self
            .intent
            .boundary
            .as_ref()
            .expect("invariant: verified terminal boundary");
        let prior = replay.finish_source(&state, Some(&boundary.snapshot_hash))?;
        let detach = match &prior {
            Some(prior) => FrozenDetachSource::capture_current(
                &state,
                self.original(),
                &boundary.snapshot_hash,
                self.intent
                    .origin
                    .predecessor()
                    .expect("invariant: replayed current namespace has predecessor"),
                prior,
                &source,
                &request.target,
            ),
            None => FrozenDetachSource::capture_state(
                &state,
                self.original(),
                Some(&boundary.snapshot_hash),
                self.intent.origin.predecessor(),
                &source,
                &request.target,
            ),
        }
        .map_err(projection_error)?;
        let namespaces = match &request.effect {
            RegistryExitEffect::Disable => {
                detach.prepare_disabled_namespaces(registry.verified_registry())
            }
            RegistryExitEffect::Detach {
                occurrence_mask_hash,
            } => detach
                .verify_mask(occurrence_mask_hash, |kind, hash| self.read(kind, hash))
                .and_then(|mask| mask.prepare_current_namespaces(registry.verified_registry())),
        }
        .map_err(projection_error)?;
        Ok(VerifiedTerminalAuthority {
            authority: self,
            registry,
            state,
            history,
            namespaces,
        })
    }
}

impl Engine {
    pub fn disk_exit_requires_management(&self) -> bool {
        self.membership_fence.as_ref().is_some_and(|fence| {
            fence
                .intent
                .effect
                .as_ref()
                .is_some_and(ExitEffect::is_terminal)
        })
    }

    pub(in crate::engine) fn prepare_terminal_authority(&mut self) -> Result<TerminalAuthority> {
        self.require_no_commit_preparation()?;
        // Completed Exit generations retain their closed I/O owners here. They
        // must survive this final-voter fence, not be mistaken for its activation.
        if !self.disk_exit_requires_management()
            || self.pending_membership.is_some()
            || self
                .disk_exit_status()?
                .is_none_or(|status| status.phase != DiskExitPhase::Fenced)
        {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "terminal preparation requires only its exact old-member fence",
            ));
        }
        let archive = self.build_managed_membership_control_archive()?;
        let fence = self
            .membership_fence
            .as_ref()
            .expect("invariant: terminal fence checked");
        let source_archive =
            PendingControlArchive::open(self, &archive.archive_path, &archive.root_hash)?;
        let (frozen, _) = source_archive.generation_base(&fence.intent)?;
        if prepare_snapshot(&frozen, MAX_METADATA)?.hash
            != prepare_snapshot(&self.state, MAX_METADATA)?.hash
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "terminal archive source differs from the live frozen state",
            ));
        }
        let prepared = self.prepare_disk_registry_base()?;
        let mut objects = BTreeMap::new();
        add_objects(&mut objects, prepared.objects())?;
        let source = prepared.verify_with(|kind, hash| read_objects(&objects, kind, hash))?;
        let effect = fence
            .intent
            .effect
            .as_ref()
            .expect("invariant: terminal fence has exit");
        let target = effect.identity(&self.config.pool_id);
        let registry_effect = match effect.action {
            MembershipExitAction::PlannedOffline => RegistryExitEffect::Disable,
            MembershipExitAction::Deregister => {
                let detach = FrozenDetachSource::capture(self, &source, &target)
                    .map_err(projection_error)?;
                let mask = detach.prepare_mask().map_err(projection_error)?;
                add_objects(&mut objects, mask.objects())?;
                RegistryExitEffect::Detach {
                    occurrence_mask_hash: mask.hash().to_owned(),
                }
            }
        };
        let successor = source.prepare_terminal_exit(RegistryExitRequest {
            target,
            operation_id: fence.intent.operation_id,
            principal: fence.intent.principal.clone(),
            request_digest: fence.hash.clone(),
            intent_hash: fence.hash.clone(),
            effect: registry_effect,
        })?;
        add_objects(&mut objects, successor.objects())?;
        let authority = TerminalAuthority {
            format: TerminalFormat::Management,
            intent: fence.intent.clone(),
            archive_root: archive.root_hash,
            predecessor: self
                .config
                .generation()
                .map(|generation| generation.portable.clone()),
            source_registry: source.commitment().clone(),
            registry: successor.commitment().clone(),
            objects,
        };
        PreparedMetadataObject::new(AUTHORITY_KIND, &authority, MAX_METADATA)?;
        authority
            .clone()
            .verify_archive(&DiskRoot::open(&self.root)?)?;
        Ok(authority)
    }
}

fn projection_error(error: ProjectionError) -> Error {
    Error::from(std::io::Error::other(error)).context(
        ErrorCode::PublicationInvalid,
        "terminal occurrence mask does not match its exact frozen source",
    )
}
fn read_objects(
    objects: &BTreeMap<String, GraphObject>,
    kind: &'static str,
    hash: &str,
) -> Result<Vec<u8>> {
    let object = objects
        .get(hash)
        .ok_or_else(|| Error::new(ErrorCode::ObjectCorrupt, "terminal graph object missing"))?;
    if object.kind != kind {
        return Err(Error::new(
            ErrorCode::ObjectCorrupt,
            "terminal graph kind differs",
        ));
    }
    Ok(json_codec::to_vec(&object.value)?)
}
fn add_objects<'a>(
    objects: &mut BTreeMap<String, GraphObject>,
    values: impl Iterator<Item = (&'static str, &'a str, &'a [u8])>,
) -> Result<()> {
    for (kind, hash, bytes) in values {
        let object = GraphObject {
            kind: kind.to_owned(),
            value: decode_strict(bytes)?,
        };
        if let Some(previous) = objects.insert(hash.to_owned(), object.clone())
            && (previous.kind != object.kind || previous.value != object.value)
        {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "terminal graph hash collision",
            ));
        }
    }
    Ok(())
}
