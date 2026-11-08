//! Complete native first-slot inputs for one already-selected terminal chain.
//! The encoded PUBLISHED receipt is prospective, not an observed publication.
//! This private core is deliberately not convertible to PortableAuthority/I/O.
use super::*;
use crate::engine::disk_lifecycle::terminal::retirement::VerifiedRetirementCheckpoint;

const CORE_KIND: &str = "membership-generation";
const STATE_KIND: &str = "terminal-chain-first-state";
const MAX_OBJECTS: usize = 8192;
const MAX_BYTES: usize = 128 * 1024 * 1024;

fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::PublicationInvalid, message)
}
fn budget() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "terminal-chain first slot exceeds 8192 native objects or 128 MiB",
    )
}
#[derive(Clone, Debug, Serialize, Deserialize)]
enum CoreFormat {
    #[serialization(rename = "terminal-chain-readmission-generation")]
    TerminalChain,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Core {
    format: CoreFormat,
    selection: Selection,
    target: Certificate,
}
impl Core {
    fn verify(&self, source: &VerifiedTerminalAuthority) -> Result<VerifiedRetirementCheckpoint> {
        self.selection.verify(source)?;
        self.selection.verify_certificate(&self.target)?;
        // Full typed replay includes every prior cancellation, retirement, Stop,
        // permanent operation and complete namespace/mask projection.
        let checkpoint = self.selection.proposal.activation_source(source)?;
        Ok(checkpoint)
    }
    fn first_state(&self, checkpoint: &VerifiedRetirementCheckpoint, hash: &str) -> Result<State> {
        let proposal = &self.selection.proposal;
        let source = checkpoint.state();
        if source.operations.contains_key(&proposal.operation())
            || source
                .operation_principals
                .contains_key(&proposal.operation())
        {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "terminal-chain first slot would replace a permanent operation",
            ));
        }
        let mut state = source.clone();
        state.sequence = state.sequence.checked_add(1).ok_or_else(budget)?;
        state.topology_generation = state
            .topology_generation
            .checked_add(1)
            .ok_or_else(budget)?;
        let result = OperationResult {
            operation_id: proposal.operation(),
            kind: "disk-readmit".into(),
            commit_id: None,
            history_tree_id: None,
            branch_id: None,
            commit_state: storage_contract::CommitState::Published,
            protection_at_publish: storage_contract::ProtectionAtPublish::NotApplicable,
            current_health: storage_contract::CurrentHealth::Unknown,
            request_digest: self.selection.hash()?,
            sequence: state.sequence,
            result: json_codec::json!({
                "authority_kind": "terminal-chain-readmission",
                "generation": hash,
                "terminal_authority": self.selection.terminal.hash()?,
                "checkpoint": checkpoint.digest(),
                "disk_id": proposal.target().disk_id,
                "slot_id": proposal.target().slot_id,
                "binding_generation": proposal.target().binding_generation.to_string(),
                "registration": "enabled",
                "topology_generation": state.topology_generation.to_string(),
                "metadata_members": 1,
            }),
        };
        state.operations.insert(proposal.operation(), result);
        state
            .operation_principals
            .insert(proposal.operation(), proposal.principal().into());
        Ok(state)
    }
}

/// Exact selected bytes, not a health claim or a completion certificate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Manifest {
    pub(super) core_hash: String,
    pub(super) candidate_hash: String,
    pub(super) snapshot_hash: String,
    first_state_hash: String,
    pub(super) selection_hash: String,
    checkpoint_hash: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub(super) sequence: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub(super) topology_generation: u64,
    pub(super) native_objects: usize,
    #[serialization(with = "storage_contract::u64_string")]
    pub(super) native_bytes: u64,
}
struct NativeObject {
    kind: String,
    hash: String,
    bytes: Vec<u8>,
}
pub(super) struct Prepared {
    core: Core,
    candidate: Candidate,
    result: OperationResult,
    manifest: Manifest,
    files: BTreeMap<PathBuf, NativeObject>,
}
impl Prepared {
    pub(super) fn new(
        selection: &Selection,
        certificate: &Certificate,
        source: &VerifiedTerminalAuthority,
    ) -> Result<Self> {
        let core = Core {
            format: CoreFormat::TerminalChain,
            selection: selection.clone(),
            target: certificate.clone(),
        };
        let checkpoint = core.verify(source)?;
        let object = PreparedMetadataObject::new(CORE_KIND, &core, MAX_METADATA)?;
        let state = core.first_state(&checkpoint, &object.hash)?;
        let snapshot = prepare_snapshot(&state, MAX_METADATA)?;
        let (history, history_objects) =
            crate::history_package::prepare_history_manifests(&state, MAX_METADATA)?;
        let proposal = &selection.proposal;
        let candidate = Candidate::new_with_registry(
            CandidateData {
                pool_id: proposal.configuration().pool_id.clone(),
                sequence: state.sequence,
                transaction_id: format!("tx_{}", &object.hash[..32]),
                operation_id: proposal.operation(),
                request_digest: selection.hash()?,
                // Terminal management has no new business publication. The
                // original published snapshot remains the exact predecessor.
                previous_snapshot: Some(
                    selection
                        .terminal
                        .intent()
                        .boundary
                        .as_ref()
                        .expect("invariant: verified terminal authority has published source")
                        .snapshot_hash
                        .clone(),
                ),
                snapshot_hash: snapshot.hash.clone(),
                versions: Vec::new(),
                metadata_disk_ids: vec![proposal.target().disk_id.clone()],
                history_roots: history.reference().clone(),
            },
            &object.hash,
            &proposal.preparation()?.proposed_registry,
        );
        let candidate_object = PreparedMetadataObject::new("candidate", &candidate, MAX_METADATA)?;
        let mut prepared = Self {
            manifest: Manifest {
                core_hash: object.hash.clone(),
                candidate_hash: candidate_object.hash.clone(),
                snapshot_hash: snapshot.hash.clone(),
                first_state_hash: object_hash(STATE_KIND, &state)?,
                selection_hash: selection.hash()?,
                checkpoint_hash: checkpoint.digest().into(),
                sequence: state.sequence,
                topology_generation: state.topology_generation,
                native_objects: 0,
                native_bytes: 0,
            },
            core,
            result: state
                .operations
                .get(&proposal.operation())
                .expect("invariant: prescribed first result")
                .clone(),
            candidate,
            files: BTreeMap::new(),
        };
        prepared.add(object.kind(), &object.hash, object.bytes())?;
        snapshot
            .visit_prepared_objects(&mut |kind, hash, bytes| prepared.add(kind, hash, bytes))?;
        for object in history_objects {
            prepared.add(object.kind(), &object.hash, object.bytes())?;
        }
        proposal
            .visit_registry_objects(&mut |kind, hash, bytes| prepared.add(kind, hash, bytes))?;
        prepared.add(
            candidate_object.kind(),
            &candidate_object.hash,
            candidate_object.bytes(),
        )?;
        PreparedMetadataObject::new(
            "terminal-chain-first-inputs",
            &prepared.manifest,
            MAX_METADATA,
        )?;
        Ok(prepared)
    }
    fn add(&mut self, kind: &str, hash: &str, bytes: &[u8]) -> Result<()> {
        let path = address(kind, hash)?;
        if bytes.len() > MAX_METADATA {
            return Err(budget());
        }
        if let Some(prior) = self.files.get(&path) {
            if prior.bytes != bytes {
                return Err(invalid("terminal-chain native object address collision"));
            }
            return Ok(());
        }
        let total = self
            .manifest
            .native_bytes
            .checked_add(bytes.len() as u64)
            .ok_or_else(budget)?;
        if self.files.len() == MAX_OBJECTS || total > MAX_BYTES as u64 {
            return Err(budget());
        }
        self.files.insert(
            path,
            NativeObject {
                kind: kind.into(),
                hash: hash.into(),
                bytes: bytes.into(),
            },
        );
        self.manifest.native_objects = self.files.len();
        self.manifest.native_bytes = total;
        Ok(())
    }
    pub(super) fn disk(&self) -> &DiskConfig {
        self.core.selection.proposal.target_disk()
    }
    pub(super) fn candidate(&self) -> &Candidate {
        &self.candidate
    }
    pub(super) fn result(&self) -> &OperationResult {
        &self.result
    }
    pub(super) fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    pub(super) fn require_capacity(&self, root: &DiskRoot) -> Result<()> {
        // Count the entire local bundle, an atomic temporary, both manifests
        // and allocation overhead. No other root's free space can satisfy it.
        let required = self.manifest.native_bytes * 2
            + 2 * MAX_METADATA as u64
            + 16 * 1024 * 1024
            + self.files.len() as u64 * 8192;
        if root.available()? < required {
            return Err(Error::new(
                ErrorCode::NoSpace,
                "SYSTEM and original target each need a complete isolated first-slot bundle",
            ));
        }
        Ok(())
    }
    pub(super) fn install(&self, root: &DiskRoot) -> Result<()> {
        for (path, object) in &self.files {
            let path = root.path().join(path);
            root.private_dir(path.parent().expect("invariant: native object has parent"))?;
            root.write_atomic(&path, &object.bytes, false)?;
        }
        self.verify_at(root)
    }
    pub(super) fn verify_at(&self, root: &DiskRoot) -> Result<()> {
        let mut budget = ReadBudget::default();
        // Equality to the typed source recipe is essential; self-consistent
        // replaced State/Candidate bytes alone are never sufficient authority.
        let native_core = budget.read(root, CORE_KIND, &self.manifest.core_hash)?;
        let saved: Core = decode_strict(native_core.bytes())?;
        if object_hash(CORE_KIND, &saved)? != object_hash(CORE_KIND, &self.core)? {
            return Err(invalid(
                "terminal-chain native core differs from selected actual vote",
            ));
        }
        let object = budget.read(root, "candidate", &self.manifest.candidate_hash)?;
        let candidate: Candidate = crate::journal::decode_proof(object.bytes())?;
        let expected = &self.files[&address("candidate", &self.manifest.candidate_hash)?];
        if object.bytes() != expected.bytes.as_slice() {
            return Err(invalid(
                "terminal-chain first candidate differs from exact source recipe",
            ));
        }
        let snapshot = budget.read(root, "snapshot", &candidate.snapshot_hash)?;
        let snapshot: crate::journal::Snapshot = json_codec::from_value(snapshot.into_value())?;
        let state = crate::journal::resolve_admitted_snapshot(snapshot, |kind, hash| {
            budget.read(root, kind, hash)
        })?;
        if object_hash(STATE_KIND, state.state())? != self.manifest.first_state_hash {
            return Err(invalid(
                "terminal-chain first State lost complete permanent history",
            ));
        }
        for (path, object) in &self.files {
            budget.charge(object.bytes.len())?;
            let actual = root.read_bytes(&root.path().join(path), object.bytes.len())?;
            if actual != object.bytes {
                return Err(invalid(
                    "terminal-chain first-slot dependency bytes changed",
                ));
            }
            let value: json_codec::Value = decode_strict(&actual)?;
            if object_hash(&object.kind, &value)? != object.hash {
                return Err(invalid("terminal-chain native dependency digest changed"));
            }
        }
        Ok(())
    }
}
pub(super) fn address(kind: &str, hash: &str) -> Result<PathBuf> {
    if kind.is_empty()
        || !kind.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
        || hash.len() != 64
        || !hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid("invalid terminal-chain native object address"));
    }
    Ok(PathBuf::from("objects")
        .join(kind)
        .join(&hash[..2])
        .join(format!("{hash}.json")))
}
#[derive(Default)]
struct ReadBudget {
    objects: usize,
    bytes: usize,
}
impl ReadBudget {
    fn charge(&mut self, bytes: usize) -> Result<()> {
        self.objects += 1;
        self.bytes = self.bytes.checked_add(bytes).ok_or_else(budget)?;
        if self.objects > MAX_OBJECTS * 3 || self.bytes > MAX_BYTES * 3 {
            return Err(budget());
        }
        Ok(())
    }
    fn read(
        &mut self,
        root: &DiskRoot,
        kind: &'static str,
        hash: &str,
    ) -> Result<AuthenticatedMetadataObject> {
        let path = address(kind, hash)?;
        let bytes = root.read_bytes(&root.path().join(path), MAX_METADATA)?;
        self.charge(bytes.len())?;
        AuthenticatedMetadataObject::authenticate(
            kind,
            hash,
            storage_contract::strict_json(&bytes)?,
        )
    }
}
