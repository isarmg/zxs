//! Portable evidence for a completed management chain. It preserves every
//! selected frame, exact Stop bytes, complete mask and permanent receipt.
//! This checkpoint has no method that admits active registry or storage I/O.
use super::*;
use crate::engine::disk_lifecycle::bootstrap::store::RetirementStopSnapshot;
use crate::engine::membership_transition::terminal_resume::cancellation::{
    self, ResumeCancellation,
};
use data_serialization::de::DeserializeOwned;

const KIND: &str = "terminal-retirement-chain-checkpoint";
const STATE_KIND: &str = "terminal-retirement-checkpoint-state";
#[derive(Clone, Debug, Serialize, Deserialize)]
enum ArchiveFormat {
    #[serialization(rename = "terminal-retirement-chain-checkpoint")]
    CompletedChain,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum StopSnapshot {
    Fresh {
        binding: StopPlan,
        native: RetirementStopSnapshot,
    },
    Existing {
        native: stopped::ExistingStop,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Frame {
    predecessor: Option<chain::Next>,
    intent: Intent,
    selected: Selection,
    successor: Successor,
    stop: StopSnapshot,
    completed: Completion,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Boundary {
    terminal_authority_hash: String,
    control_archive_root_hash: String,
    registry: DiskRegistryCommitment,
    last_completion_hash: String,
    state_hash: String,
    #[serialization(with = "storage_contract::u64_string")]
    sequence: u64,
    #[serialization(with = "storage_contract::u64_string")]
    topology_generation: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::engine) struct RetirementCheckpointProof {
    format: ArchiveFormat,
    cancelled: Vec<ResumeCancellation>,
    frames: Vec<Frame>,
    checkpoint: Boundary,
}

/// Checkpoint report, not a new resume authority or a claim of live D3 on a
/// different process/host. The trusted digest must be retained independently.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct TerminalRetirementCheckpoint {
    pub digest: String,
    pub terminal_authority_hash: String,
    pub control_archive_root_hash: String,
    pub registry: DiskRegistryCommitment,
    pub state_hash: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub sequence: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub topology_generation: u64,
    pub completed_retirements: usize,
    pub prior_cancelled_attempts: usize,
    pub metadata_resumption_supported: bool,
    pub active_namespace_admitted: bool,
    pub target_writer_admitted: bool,
    pub live_io_exclusion_transferred: bool,
}

/// Encoded output from actual verified SYSTEM material. Reading these bytes
/// never registers a directory, changes a locator or writes a source file.
pub struct TerminalRetirementArchive {
    bytes: Vec<u8>,
    checkpoint: TerminalRetirementCheckpoint,
}
impl TerminalRetirementArchive {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn checkpoint(&self) -> &TerminalRetirementCheckpoint {
        &self.checkpoint
    }
}

/// Only the complete original-source replay constructs this token. It grants
/// graph preparation, never a voter decision, namespace admission or live D3.
pub(in crate::engine) struct VerifiedRetirementCheckpoint {
    projection: Projection,
    digest: String,
}
impl VerifiedRetirementCheckpoint {
    pub(in crate::engine) fn registry(&self) -> &TerminalRegistry {
        &self.projection.registry
    }
    pub(in crate::engine) fn state(&self) -> &State {
        &self.projection.state
    }
    pub(in crate::engine) fn namespaces(&self) -> &PreparedCurrentNamespaces {
        &self.projection.namespaces
    }
    pub(in crate::engine) fn digest(&self) -> &str {
        &self.digest
    }
}
struct Projection {
    registry: TerminalRegistry,
    namespaces: PreparedCurrentNamespaces,
    state: Arc<State>,
    last_completion_hash: String,
}
impl Projection {
    fn new(source: &VerifiedTerminalAuthority, cancelled: &[ResumeCancellation]) -> Result<Self> {
        cancellation::verify_history(&source.authority, cancelled, None)?;
        for cancellation in cancelled {
            cancellation.verify_frozen_source(&source.state)?;
        }
        let mut state = source.authority.management_state(&source.state)?.0;
        cancellation::apply_history(
            Arc::get_mut(&mut state)
                .expect("invariant: newly projected management state has one owner"),
            cancelled,
        )?;
        Ok(Self {
            registry: source.registry.clone(),
            namespaces: source.namespaces.clone(),
            state,
            last_completion_hash: String::new(),
        })
    }
    fn append(
        &mut self,
        source: &VerifiedTerminalAuthority,
        frame: &Frame,
        first: bool,
    ) -> Result<()> {
        let digest = object_hash(DIGEST_KIND, &frame.intent)?;
        match (&frame.predecessor, first) {
            (None, true) => {}
            (Some(link), false)
                if link.predecessor_completion == self.last_completion_hash
                    && link.intent == frame.intent
                    && link.intent_digest == digest => {}
            _ => {
                return Err(failure(
                    ErrorCode::LogFork,
                    "portable retirement omitted, reordered or changed an exact predecessor link",
                ));
            }
        }
        validate_at(
            &source.authority,
            &self.registry,
            &self.state,
            &frame.intent,
        )?;
        if frame.selected.operation_id != frame.intent.operation_id
            || frame.selected.intent_digest != digest
        {
            return Err(failure(
                ErrorCode::PublicationInvalid,
                "portable retirement lacks its exact original selection",
            ));
        }
        let plan = TerminalRetirementPlan {
            intent: frame.intent.clone(),
            digest,
        };
        let next = verify_successor(
            source,
            &self.registry,
            &self.namespaces,
            &plan,
            &frame.successor,
        )?;
        frame
            .stop
            .verify(&source.authority, &self.registry, &plan, &frame.successor)?;
        let expected = Completion {
            format: Format::Retirement,
            intent_digest: plan.digest.clone(),
            successor_hash: frame.successor.hash()?,
            result: retirement_result_for(&plan, &frame.successor)?,
        };
        if frame.completed != expected {
            return Err(failure(
                ErrorCode::LogFork,
                "portable retirement permanent receipt changed",
            ));
        }
        self.last_completion_hash = object_hash("terminal-retirement-completion", &expected)?;
        let state = Arc::make_mut(&mut self.state);
        state.sequence = expected.result.sequence;
        state.topology_generation = state.topology_generation.checked_add(1).ok_or_else(|| {
            failure(
                ErrorCode::ResourceLimit,
                "portable retirement topology exhausted",
            )
        })?;
        state
            .operation_principals
            .insert(plan.operation_id(), plan.principal().into());
        state
            .operations
            .insert(plan.operation_id(), expected.result);
        self.registry = next.registry;
        self.namespaces = next.namespaces;
        Ok(())
    }
    fn boundary(&self, source: &VerifiedTerminalAuthority) -> Result<Boundary> {
        Ok(Boundary {
            terminal_authority_hash: source.authority.hash()?,
            control_archive_root_hash: source.authority.archive_root_hash().into(),
            registry: self.registry.commitment().clone(),
            last_completion_hash: self.last_completion_hash.clone(),
            state_hash: object_hash(STATE_KIND, &*self.state)?,
            sequence: self.state.sequence,
            topology_generation: self.state.topology_generation,
        })
    }
}
impl StopSnapshot {
    fn verify(
        &self,
        authority: &TerminalAuthority,
        registry: &TerminalRegistry,
        plan: &TerminalRetirementPlan,
        successor: &Successor,
    ) -> Result<()> {
        match self {
            Self::Existing { native } => {
                stopped::verify_portable_existing_stop(native, authority, registry, plan, successor)
            }
            Self::Fresh { binding, native } => {
                require_stop(binding, plan, successor)?;
                native
                    .verify(
                        plan.target(),
                        plan.operation_id(),
                        IntentDigest::parse(plan.digest()).map_err(management_error)?,
                        authority
                            .io_admission_epoch(&plan.target().disk_id)?
                            .as_deref(),
                    )
                    .map_err(management_error)?;
                // Both fields must retain the very same original stop plan.
                native
                    .require_same_plan(&binding.stop)
                    .map_err(management_error)
            }
        }
    }
}
impl RetirementCheckpointProof {
    pub(in crate::engine) fn verify(
        &self,
        source: &VerifiedTerminalAuthority,
    ) -> Result<VerifiedRetirementCheckpoint> {
        if self.frames.is_empty() {
            return Err(failure(
                ErrorCode::PublicationInvalid,
                "retirement checkpoint cannot omit the completed chain",
            ));
        }
        let mut projection = Projection::new(source, &self.cancelled)?;
        for (index, frame) in self.frames.iter().enumerate() {
            projection.append(source, frame, index == 0)?;
        }
        if projection.boundary(source)? != self.checkpoint {
            return Err(failure(
                ErrorCode::LogFork,
                "retirement checkpoint differs from its complete frozen-source projection",
            ));
        }
        Ok(VerifiedRetirementCheckpoint {
            projection,
            digest: object_hash(KIND, self)?,
        })
    }
    pub(in crate::engine) fn report(&self, digest: String) -> TerminalRetirementCheckpoint {
        TerminalRetirementCheckpoint {
            digest,
            terminal_authority_hash: self.checkpoint.terminal_authority_hash.clone(),
            control_archive_root_hash: self.checkpoint.control_archive_root_hash.clone(),
            registry: self.checkpoint.registry.clone(),
            state_hash: self.checkpoint.state_hash.clone(),
            sequence: self.checkpoint.sequence,
            topology_generation: self.checkpoint.topology_generation,
            completed_retirements: self.frames.len(),
            prior_cancelled_attempts: self.cancelled.len(),
            metadata_resumption_supported: false,
            active_namespace_admitted: false,
            target_writer_admitted: false,
            live_io_exclusion_transferred: false,
        }
    }
}
impl TerminalManagement {
    /// Capture only the complete current chain. Unfinished links/operations fail;
    /// there is no prefix-as-latest fallback and no data-root open in this method.
    pub fn export_retirement_archive(&self) -> Result<TerminalRetirementArchive> {
        self.require_no_chain_readmission()?;
        let (wire, _) = self.current_retirement_checkpoint()?;
        let object = PreparedMetadataObject::new(KIND, &wire, MAX_METADATA)?;
        Ok(TerminalRetirementArchive {
            checkpoint: wire.report(object.hash.clone()),
            bytes: object.bytes().to_vec(),
        })
    }

    pub(in crate::engine) fn current_retirement_checkpoint(
        &self,
    ) -> Result<(RetirementCheckpointProof, VerifiedRetirementCheckpoint)> {
        self.store.verify_writer(&self._locks)?;
        self.require_no_pending_retirement()?;
        let completed = self.retirement_completion.as_ref().ok_or_else(|| {
            failure(
                ErrorCode::NotFound,
                "no completed terminal retirement chain to export",
            )
        })?;
        let attempts = cancellation::attempts::load(self.system_root(), &self.verified)?;
        if attempts.has_current() {
            return Err(failure(
                ErrorCode::RecoveryRequired,
                "a pending metadata resume cannot be exported as a settled retirement checkpoint",
            ));
        }
        let checkpoint = Boundary {
            terminal_authority_hash: self.verified.authority.hash()?,
            control_archive_root_hash: self.verified.authority.archive_root_hash().into(),
            registry: self.registry().commitment().clone(),
            last_completion_hash: completed.hash().into(),
            state_hash: object_hash(STATE_KIND, &*self.state)?,
            sequence: self.state.sequence,
            topology_generation: self.state.topology_generation,
        };
        let mut wire = RetirementCheckpointProof {
            format: ArchiveFormat::CompletedChain,
            cancelled: attempts.cancelled,
            frames: Vec::new(),
            checkpoint,
        };
        let mut budget = json_codec::to_vec(&wire)?.len();
        if budget > MAX_METADATA {
            return Err(failure(
                ErrorCode::ResourceLimit,
                "retirement checkpoint cancellation history exceeds metadata budget",
            ));
        }
        let mut directory = self
            .store
            .operation()
            .private_child(self.store.operation().path())?;
        let mut predecessor = None;
        let count = self
            .retirement_history
            .len()
            .checked_add(1)
            .ok_or_else(|| failure(ErrorCode::ResourceLimit, "retirement frame count overflow"))?;
        let bootstrap = open_store(self.system_root().path())?;
        for index in 0..count {
            let frame = read_frame(&directory, predecessor, &bootstrap)?;
            let frame_bytes = json_codec::to_vec(&frame)?.len();
            budget = budget
                .checked_add(frame_bytes)
                .and_then(|n| n.checked_add(1))
                .ok_or_else(|| {
                    failure(
                        ErrorCode::ResourceLimit,
                        "retirement checkpoint size overflow",
                    )
                })?;
            if budget > MAX_METADATA {
                return Err(failure(
                    ErrorCode::ResourceLimit,
                    "complete retirement checkpoint exceeds metadata budget; no prefix exported",
                ));
            }
            let next: Option<chain::Next> =
                store::optional(&directory, &directory.path().join(chain::NEXT))?;
            wire.frames.push(frame);
            match (next, index + 1 == count) {
                (None, true) => break,
                (Some(next), false) => {
                    let path = self
                        .store
                        .operation()
                        .path()
                        .join(chain::OPERATIONS)
                        .join(next.intent.operation_id.to_string());
                    directory = self.store.operation().private_child(&path)?;
                    predecessor = Some(next);
                }
                _ => {
                    return Err(failure(
                        ErrorCode::LogFork,
                        "retirement chain changed during checkpoint capture",
                    ));
                }
            }
        }
        let verified = wire.verify(&self.verified)?;
        self.store.verify_writer(&self._locks)?;
        PreparedMetadataObject::new(KIND, &wire, MAX_METADATA)?;
        Ok((wire, verified))
    }
    /// Verify portable checkpoint bytes against this already authenticated frozen
    /// original archive and an independently retained digest. Does not assert
    /// that this old checkpoint is latest, or mutate any authority/locator.
    pub fn verify_retirement_archive(
        &self,
        bytes: &[u8],
        expected_digest: &str,
    ) -> Result<TerminalRetirementCheckpoint> {
        self.store.verify_writer(&self._locks)?;
        expected_digest
            .parse::<storage_contract::ContentHash>()
            .map_err(|error| {
                Error::from(std::io::Error::other(error)).context(
                    ErrorCode::ObjectCorrupt,
                    "invalid trusted retirement checkpoint digest",
                )
            })?;
        if bytes.len() > MAX_METADATA {
            return Err(failure(
                ErrorCode::ResourceLimit,
                "retirement checkpoint exceeds metadata budget",
            ));
        }
        let wire: RetirementCheckpointProof = crate::decode_strict(bytes)?;
        let object = PreparedMetadataObject::new(KIND, &wire, MAX_METADATA)?;
        if object.hash != expected_digest {
            return Err(failure(
                ErrorCode::ObjectCorrupt,
                "retirement checkpoint differs from its independently retained digest",
            ));
        }
        wire.verify(&self.verified)?;
        Ok(wire.report(expected_digest.into()))
    }
}
fn required<T: DeserializeOwned>(directory: &DiskRoot, name: &str) -> Result<T> {
    store::optional(directory, &directory.path().join(name))?.ok_or_else(|| {
        failure(
            ErrorCode::ObjectCorrupt,
            "completed retirement checkpoint lost a required native file",
        )
    })
}
fn read_frame(
    directory: &DiskRoot,
    predecessor: Option<chain::Next>,
    bootstrap: &crate::engine::disk_lifecycle::bootstrap::store::BootstrapStore,
) -> Result<Frame> {
    let fresh: Option<StopPlan> = store::optional(directory, &directory.path().join(STOP))?;
    let existing: Option<stopped::ExistingStop> =
        store::optional(directory, &directory.path().join(EXISTING))?;
    let stop = match (fresh, existing) {
        (Some(binding), None) => StopSnapshot::Fresh {
            native: bootstrap
                .retirement_stop_snapshot(&binding.stop)
                .map_err(management_error)?,
            binding,
        },
        (None, Some(native)) => StopSnapshot::Existing { native },
        _ => {
            return Err(failure(
                ErrorCode::PublicationInvalid,
                "retirement checkpoint requires exactly one original Stop proof",
            ));
        }
    };
    Ok(Frame {
        predecessor,
        intent: required(directory, INTENT)?,
        selected: required(directory, SELECTED)?,
        successor: required(directory, SUCCESSOR)?,
        stop,
        completed: required(directory, COMPLETE)?,
    })
}
