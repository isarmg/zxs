use crate::{disk::*, *};
use json_codec::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

mod config;
mod index;
mod observations;
mod operations;
pub use observations::{DiskObservation, DiskObservationPlan};
pub use operations::{OperationReadPlan, OperationRecord};
mod publication;
pub(crate) use publication::DetachedPublication;
mod workspace_commit;
pub use workspace_commit::{PreparedWorkspaceCommit, WorkspaceCommitPreparation};
mod reads;
pub use reads::{ContentReadPlan, RangeContent, WorkspaceReadPlan};
mod recovery;
mod recovery_checkpoint;
mod workspace_budget;
mod workspace_limits;
pub use workspace_limits::RetainedWorkspaceLimits;
mod workspace_storage;
use config::{acquire_locks, validate_config};

pub struct Engine {
    pub(crate) root: PathBuf,
    pub(crate) config: Config,
    pub(crate) state: Arc<State>,
    pub(crate) workspaces: BTreeMap<String, Workspace>,
    workspace_budget: workspace_budget::WorkspaceBudget,
    retained_workspace_limits: Option<RetainedWorkspaceLimits>,
    pub(crate) blocked: bool,
    pub(crate) snapshot: Option<String>,
    published_history_base: Option<Arc<publication::PublishedHistoryBase>>,
    pub(crate) next_slot: u64,
    pub(crate) reads: Arc<Mutex<BTreeMap<String, usize>>>,
    pub(crate) upload_preparations: Arc<Mutex<BTreeSet<String>>>,
    commit_preparation: Arc<std::sync::atomic::AtomicBool>,
    history_query_proof: Mutex<index::QueryProof>,
    _locks: Vec<File>,
    pub(crate) fault: Option<String>,
}
pub struct ReadLease {
    version: String,
    reads: Arc<Mutex<BTreeMap<String, usize>>>,
}
impl Drop for ReadLease {
    fn drop(&mut self) {
        if let Ok(mut reads) = self.reads.lock()
            && let Some(count) = reads.get_mut(&self.version)
        {
            *count = count.saturating_sub(1);
            if *count == 0 {
                reads.remove(&self.version);
            }
        }
    }
}
pub struct OpenedContent {
    pub file: File,
    pub size: u64,
    pub content_hash: String,
    pub lease: ReadLease,
}
pub(crate) struct Prepared {
    pub version: FileVersion,
    pub placement: Placement,
    pub source: PathBuf,
}

/// A real unpublished mutation plan; constructing it never acknowledges a
/// publication or writes a decision record.
pub(crate) enum MutationPlan {
    Existing(OperationResult),
    Prepared {
        next: Box<State>,
        prepared: Vec<Prepared>,
        result: OperationResult,
    },
}

impl Engine {
    /// Detached filesystem work must keep the same OS lock descriptions alive
    /// until completion or cancellation, even if its original Engine is dropped.
    pub(crate) fn retain_writer_locks(&self) -> Result<Vec<File>> {
        Ok(self
            ._locks
            .iter()
            .map(File::try_clone)
            .collect::<std::io::Result<Vec<_>>>()?)
    }

    /// Validate a candidate without creating directories, opening disks or
    /// acquiring writer locks. Filesystem identity is checked when opening it.
    pub fn validate_configuration(config: &Config) -> Result<()> {
        validate_config(config)
    }

    /// Candidate membership is immutable and must never shrink with availability.
    pub(crate) fn verify_candidate_membership(
        &self,
        candidate: &crate::journal::Candidate,
    ) -> Result<()> {
        let expected: BTreeSet<_> = self.members().into_iter().map(|d| d.disk_id).collect();
        let actual: BTreeSet<_> = candidate.metadata_disk_ids.iter().cloned().collect();
        if expected.is_empty()
            || actual != expected
            || actual.len() != candidate.metadata_disk_ids.len()
        {
            return Err(Error::new(
                "PUBLICATION_INVALID",
                "candidate membership differs from configured metadata members",
            ));
        }
        Ok(())
    }

    pub fn initialize(root: &Path, config: Config) -> Result<Self> {
        Self::initialize_with_workspace_limits(root, config, None)
    }

    /// Set restart-only retained-evidence limits without changing disk configuration.
    pub fn initialize_with_workspace_limits(
        root: &Path,
        mut config: Config,
        workspace_limits: Option<RetainedWorkspaceLimits>,
    ) -> Result<Self> {
        if root.join("config.json").exists() {
            return Err(Error::new(
                "ALREADY_INITIALIZED",
                "configuration already exists; use open/recovery",
            ));
        }
        validate_config(&config)?;
        if let Some(limits) = workspace_limits {
            limits.validate_for(&config)?;
        }
        private_dir(root)?;
        let root = existing_private_dir(root)?.path().to_path_buf();
        for d in &mut config.disks {
            private_dir(&d.root)?;
            d.root = d.root.canonicalize()?;
            if d.root.join("disk.json").exists() {
                return Err(Error::new(
                    "DISK_ALREADY_ENROLLED",
                    "disk already contains a managed identity",
                ));
            }
        }
        validate_config(&config)?;
        let mut physical = BTreeSet::new();
        for d in &config.disks {
            let seal = identity(&config, d)?;
            if !physical.insert(seal.physical_device.clone()) {
                return Err(Error::new(
                    "DISK_NOT_INDEPENDENT",
                    "two slots reference one physical disk",
                ));
            }
        }
        let locks = acquire_locks(&root, &config, false)?;
        for d in &config.disks {
            let seal = identity(&config, d)?;
            write_json(&d.root.join("disk.json"), &seal, false)?;
            write_atomic(&d.root.join("README.txt"),b"ZXS disk format 2. Ordinary full files are stored below histories/. Use zxs-rescue offline-check/offline-export before recovery. Development disks are not physical redundancy evidence.\n",false)?;
            for p in [
                "histories",
                "metadata/application-log",
                "metadata/objects",
                "staging",
                "quarantine",
            ] {
                private_dir(&d.root.join(p))?;
            }
            sync_dir(&d.root)?;
        }
        // Every payload disk retains the fixed membership anchor for offline proofs.
        for d in &config.disks {
            write_json(
                &d.root.join("metadata/pool-membership/config.json"),
                &config,
                false,
            )?;
        }
        write_json(&root.join("config.json"), &config, false)?;
        private_dir(&root.join("workspaces"))?;
        let state = State {
            topology_generation: 1,
            ..State::default()
        };
        let engine = Self {
            root,
            config,
            state: Arc::new(state),
            workspaces: BTreeMap::new(),
            workspace_budget: workspace_budget::WorkspaceBudget::default(),
            retained_workspace_limits: workspace_limits,
            blocked: false,
            snapshot: None,
            published_history_base: None,
            next_slot: 1,
            reads: Arc::new(Mutex::new(BTreeMap::new())),
            upload_preparations: Arc::new(Mutex::new(BTreeSet::new())),
            commit_preparation: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            history_query_proof: Mutex::new(index::QueryProof::default()),
            _locks: locks,
            fault: None,
        };
        engine.rebuild_index()?;
        Ok(engine)
    }
    pub fn open(root: &Path) -> Result<Self> {
        Self::open_with_workspace_limits(root, None)
    }

    pub fn open_with_workspace_limits(
        root: &Path,
        workspace_limits: Option<RetainedWorkspaceLimits>,
    ) -> Result<Self> {
        Self::open_mode(root, true, workspace_limits)
    }

    /// Offline recovery entry. Unpublished candidates are never advanced here.
    pub fn open_for_recovery(root: &Path) -> Result<Self> {
        Self::open_for_recovery_with_workspace_limits(root, None)
    }

    pub fn open_for_recovery_with_workspace_limits(
        root: &Path,
        workspace_limits: Option<RetainedWorkspaceLimits>,
    ) -> Result<Self> {
        Self::open_mode(root, false, workspace_limits)
    }

    fn open_mode(
        root: &Path,
        complete_unpublished: bool,
        workspace_limits: Option<RetainedWorkspaceLimits>,
    ) -> Result<Self> {
        let root = existing_private_dir(root)?.path().to_path_buf();
        let config: Config = read_json(&root.join("config.json"))?;
        validate_config(&config)?;
        if let Some(limits) = workspace_limits {
            limits.validate_for(&config)?;
        }
        let mut engine = Self::prepare_workspace_recovery(root, config, workspace_limits)?;
        engine.recover_mode(complete_unpublished)?;
        engine.load_workspaces()?;
        engine.rebuild_index()?;
        Ok(engine)
    }

    /// Admission preflight is read-only apart from writer lock files. Keep this
    /// same Engine and its locks through replay and authoritative reconciliation.
    fn prepare_workspace_recovery(
        root: PathBuf,
        config: Config,
        workspace_limits: Option<RetainedWorkspaceLimits>,
    ) -> Result<Self> {
        let locks = acquire_locks(&root, &config, true)?;
        let mut engine = Self {
            root,
            config,
            state: Arc::new(State {
                topology_generation: 1,
                ..State::default()
            }),
            workspaces: BTreeMap::new(),
            workspace_budget: workspace_budget::WorkspaceBudget::default(),
            retained_workspace_limits: workspace_limits,
            blocked: false,
            snapshot: None,
            published_history_base: None,
            next_slot: 1,
            reads: Arc::new(Mutex::new(BTreeMap::new())),
            upload_preparations: Arc::new(Mutex::new(BTreeSet::new())),
            commit_preparation: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            history_query_proof: Mutex::new(index::QueryProof::default()),
            _locks: locks,
            fault: None,
        };
        engine.seed_workspace_recovery()?;
        Ok(engine)
    }
    /// Explicit recovery after loss of the system disk. Never initializes data disks.
    pub fn restore_from_disks(root: &Path, disks: &[PathBuf]) -> Result<Self> {
        Self::restore_from_disks_with_workspace_limits(root, disks, None)
    }

    pub fn restore_from_disks_with_workspace_limits(
        root: &Path,
        disks: &[PathBuf],
        workspace_limits: Option<RetainedWorkspaceLimits>,
    ) -> Result<Self> {
        if root.join("config.json").exists() {
            return Err(Error::new(
                "ALREADY_INITIALIZED",
                "restore requires a new system state directory",
            ));
        }
        if disks.is_empty() {
            return Err(Error::new(
                "METADATA_QUORUM_UNAVAILABLE",
                "restore requires trusted old metadata members",
            ));
        }
        let mut configs = BTreeMap::<String, (Config, usize)>::new();
        let mut seals = BTreeMap::new();
        for path in disks {
            let seal: DiskSeal = read_json(&path.join("disk.json"))?;
            let config: Config = read_json(&path.join("metadata/pool-membership/config.json"))?;
            validate_config(&config)?;
            let mut member = config
                .disks
                .iter()
                .find(|disk| disk.disk_id == seal.disk_id && disk.metadata_member)
                .cloned()
                .ok_or_else(|| {
                    Error::new(
                        "CONFIG_CONFLICT",
                        "recovery input is not a metadata member of its saved configuration",
                    )
                })?;
            member.root = path.canonicalize()?;
            validate_seal(&config, &member)?;
            let hash = object_hash("pool-config", &config)?;
            configs
                .entry(hash)
                .and_modify(|(_, n)| *n += 1)
                .or_insert((config, 1));
            if seals.insert(seal.disk_id, path.canonicalize()?).is_some() {
                return Err(Error::new(
                    "DISK_NOT_INDEPENDENT",
                    "duplicate disk id in recovery input",
                ));
            }
        }
        let mut agreed = configs
            .into_values()
            .filter(|(c, n)| *n >= c.metadata_quorum())
            .map(|(c, _)| c);
        let mut config = agreed.next().ok_or_else(|| {
            Error::new(
                "CONFIG_CONFLICT",
                "a configured metadata majority does not agree on pool configuration",
            )
        })?;
        if agreed.next().is_some()
            || seals.keys().any(|id| {
                !config
                    .disks
                    .iter()
                    .any(|disk| disk.metadata_member && &disk.disk_id == id)
            })
        {
            return Err(Error::new(
                "CONFIG_CONFLICT",
                "recovery inputs identify more than one pool configuration or membership",
            ));
        }
        for d in &mut config.disks {
            if let Some(path) = seals.get(&d.disk_id) {
                d.root = path.clone();
            }
        }
        validate_config(&config)?;
        for member in config
            .disks
            .iter()
            .filter(|disk| seals.contains_key(&disk.disk_id))
        {
            validate_seal(&config, member)?;
        }
        if let Some(limits) = workspace_limits {
            limits.validate_for(&config)?;
        }
        private_dir(root)?;
        let root = existing_private_dir(root)?.path().to_path_buf();
        let mut engine = Self::prepare_workspace_recovery(root, config, workspace_limits)?;
        // A quota failure must not publish a system configuration or replay any
        // journal repair. Preflight above retains the locks across publication.
        write_json(&engine.root.join("config.json"), &engine.config, false)?;
        engine.recover_mode(true)?;
        engine.load_workspaces()?;
        engine.rebuild_index()?;
        Ok(engine)
    }
    pub(crate) fn require_recovery_locks(&self) -> Result<()> {
        if self._locks.len() != self.config.disks.len() + 1 {
            return Err(Error::new(
                "RECOVERY_REQUIRED",
                "recovery requires every original disk and its exclusive OS writer lock",
            ));
        }
        for disk in &self.config.disks {
            validate_seal(&self.config, disk)?;
        }
        Ok(())
    }
    pub fn config(&self) -> &Config {
        &self.config
    }
    pub fn state(&self) -> &State {
        self.state.as_ref()
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn is_read_only(&self) -> bool {
        self.blocked
    }
    pub fn auth_state(&self) -> Option<json_codec::Value> {
        self.state.control_auth.clone()
    }
    /// Borrow the current authoritative control snapshot while the caller holds
    /// its engine guard. Owned recovery consumers should use `auth_state`.
    pub fn auth_state_ref(&self) -> Option<&json_codec::Value> {
        self.state.control_auth.as_ref()
    }
    /// Trusted local control-plane hook; this method is never a public route.
    pub fn persist_auth_state(
        &mut self,
        snapshot: json_codec::Value,
        principal: &str,
        operation: &str,
    ) -> Result<()> {
        if !snapshot.is_object() || canonical_json(&snapshot)?.len() > 4 * 1024 * 1024 {
            return Err(Error::new(
                "AUTH_STATE_INVALID",
                "authentication snapshot is not a bounded object",
            ));
        }
        let digest = object_hash("auth-state", &snapshot)?;
        self.mutate(
            "auth.control-state",
            principal,
            operation,
            json!({"auth_state_digest":digest}),
            |_, state, _| {
                state.control_auth = Some(snapshot);
                Ok(json!({"auth_state_digest":digest}))
            },
        )?;
        Ok(())
    }
    pub fn workspace(&self, id: &str) -> Result<Workspace> {
        self.workspaces
            .get(id)
            .cloned()
            .ok_or_else(|| Error::new("WORKSPACE_NOT_FOUND", "workspace does not exist"))
    }
    /// Deterministic injection only, never enabled by network or environment input.
    pub fn inject_fault_once(&mut self, point: &str) {
        self.fault = Some(point.to_string());
    }
    pub(crate) fn failpoint(&mut self, point: &str) -> Result<()> {
        if self.fault.as_deref() == Some(point) {
            self.fault = None;
            return Err(Error::new(
                "INJECTED_FAILURE",
                format!("interrupted at {point}"),
            ));
        }
        Ok(())
    }
    pub(crate) fn trusted_evidence_members(&self) -> Vec<DiskConfig> {
        self.members()
            .into_iter()
            .filter(|disk| validate_seal(&self.config, disk).is_ok())
            .collect()
    }
    pub(crate) fn members(&self) -> Vec<DiskConfig> {
        let mut members = self
            .config
            .disks
            .iter()
            .filter(|d| d.metadata_member)
            .cloned()
            .collect::<Vec<_>>();
        members.sort_by(|a, b| a.disk_id.cmp(&b.disk_id));
        members
    }
    pub(crate) fn disk(&self, slot: &str) -> Result<&DiskConfig> {
        self.config
            .disks
            .iter()
            .find(|d| d.slot_id.eq_ignore_ascii_case(slot))
            .ok_or_else(|| Error::new("SLOT_NOT_FOUND", "storage slot is not registered"))
    }
    pub(crate) fn route(&self, primary: &str, copies: u32) -> Result<Vec<String>> {
        let first = self.disk(primary)?;
        if first.level != 1 || copies == 0 {
            return Err(Error::new(
                "PLACEMENT_INVALID",
                "placement requires a primary slot and at least one copy",
            ));
        }
        let mut route = vec![first.slot_id.clone()];
        let mut current = first;
        while route.len() < copies as usize {
            let next = self
                .config
                .disks
                .iter()
                .find(|d| {
                    d.level == current.level + 1
                        && d.upstream_slot_id
                            .as_deref()
                            .is_some_and(|s| s.eq_ignore_ascii_case(&current.slot_id))
                })
                .ok_or_else(|| {
                    Error::new(
                        "ROUTE_INCOMPLETE",
                        "requested copies exceed the configured fixed route",
                    )
                })?;
            route.push(next.slot_id.clone());
            current = next;
        }
        Ok(route)
    }
    pub(crate) fn require_no_commit_preparation(&self) -> Result<()> {
        if self
            .commit_preparation
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(Error::new(
                "COMMIT_PREPARATION_BUSY",
                "workspace payload preparation temporarily fences other writes",
            )
            .retryable());
        }
        Ok(())
    }
    pub(crate) fn require_write(&self) -> Result<()> {
        self.require_no_commit_preparation()?;
        if self.blocked {
            return Err(Error::new(
                "RECOVERY_REQUIRED",
                "an unresolved decision or unavailable metadata member blocks writes",
            )
            .retryable());
        }
        for d in self.members() {
            validate_seal(&self.config, &d)?;
        }
        Ok(())
    }

    /// An actor is immutable audit/idempotency input, not a role or credential.
    /// Authentication is performed by the server before calling this trusted API.
    pub(crate) fn require_actor(principal: &str) -> Result<()> {
        if principal.is_empty() {
            return Err(Error::new("UNAUTHENTICATED", "operation actor is required"));
        }
        Ok(())
    }

    pub(crate) fn repository_for_history(&self, history: &str) -> Result<&Repository> {
        let h = self
            .state
            .histories
            .get(history)
            .ok_or_else(|| Error::new("HISTORY_NOT_FOUND", "history tree does not exist"))?;
        self.state
            .repositories
            .get(&h.repository_id)
            .ok_or_else(|| Error::new("STATE_INVALID", "history has no repository"))
    }
    pub(crate) fn repository_for_branch(&self, branch: &str) -> Result<&Repository> {
        let b = self
            .state
            .branches
            .get(branch)
            .ok_or_else(|| Error::new("BRANCH_NOT_FOUND", "branch does not exist"))?;
        self.repository_for_history(&b.history_tree_id)
    }
    pub(crate) fn mutate<F>(
        &mut self,
        kind: &str,
        principal: &str,
        operation_id: &str,
        input: json_codec::Value,
        change: F,
    ) -> Result<OperationResult>
    where
        F: FnOnce(&Self, &mut State, &mut Vec<Prepared>) -> Result<json_codec::Value>,
    {
        self.mutate_digest(kind, principal, operation_id, input, None, change)
    }
    pub(crate) fn mutate_digest<F>(
        &mut self,
        kind: &str,
        principal: &str,
        operation_id: &str,
        input: json_codec::Value,
        digest_override: Option<String>,
        change: F,
    ) -> Result<OperationResult>
    where
        F: FnOnce(&Self, &mut State, &mut Vec<Prepared>) -> Result<json_codec::Value>,
    {
        match self.plan_mutation_digest(
            kind,
            principal,
            operation_id,
            input,
            digest_override,
            change,
        )? {
            MutationPlan::Existing(result) => Ok(result),
            MutationPlan::Prepared {
                next,
                prepared,
                result,
            } => {
                self.persist(*next, prepared, result.clone())?;
                Ok(result)
            }
        }
    }
    pub(crate) fn plan_mutation_digest<F>(
        &self,
        kind: &str,
        principal: &str,
        operation_id: &str,
        input: json_codec::Value,
        digest_override: Option<String>,
        change: F,
    ) -> Result<MutationPlan>
    where
        F: FnOnce(&Self, &mut State, &mut Vec<Prepared>) -> Result<json_codec::Value>,
    {
        validate_id(operation_id, "op")?;
        Self::require_actor(principal)?;
        let digest = digest_override.map(Ok).unwrap_or_else(|| {
            object_hash(
                "request",
                &json!({"kind":kind,"principal":principal,"body":input}),
            )
        })?;
        if let Some(result) = self.state.operations.get(operation_id) {
            if result.request_digest == digest
                && self.operation_owner(operation_id).as_deref() == Some(principal)
            {
                return self.operation(operation_id).map(MutationPlan::Existing);
            }
            return Err(Error::new(
                "IDEMPOTENCY_KEY_REUSED",
                "operation id is already bound to different input",
            ));
        }
        self.require_write()?;
        // Published and planning views share only the immutable base. Every
        // mutation still owns an independent next State until publication.
        let mut next = self.state.as_ref().clone();
        next.begin_tracking();
        crate::container_storage::capture_names(&mut next);
        next.sequence = self.next_slot;
        if !kind.starts_with("auth.") {
            next.business_generation = next
                .business_generation
                .checked_add(1)
                .ok_or_else(|| Error::new("RESOURCE_LIMIT", "business generation exhausted"))?;
        }
        let mut prepared = Vec::new();
        let mut result = change(self, &mut next, &mut prepared)?;
        crate::container_storage::capture_names(&mut next);
        result["timestamp"] = json_codec::json!(crate::business::now().to_string());
        if let Some(branch_id) = result
            .get("branch_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            && let Some(branch) = next.branches.get(&branch_id)
        {
            result["branch_generation"] = json!(branch.generation.to_string());
        }
        result["membership_epoch"] = json!("1");
        let mut protection = Vec::new();
        for p in &prepared {
            let mut slots = Vec::new();
            for slot in &p.placement.route_slot_ids {
                let disk = self.disk(slot)?;
                slots.push(json!({"slot_id":slot,"copy_level":disk.level,"disk_id":disk.disk_id,"binding_generation":disk.binding_generation.to_string(),"state":"COMPLETE_VERIFIED"}));
            }
            protection.push(json!({"version_id":p.version.version_id,"primary_slot_id":p.placement.primary_slot_id,"target_copies":p.placement.target_copies,"required_slots":p.placement.route_slot_ids,"topology_generation":p.placement.topology_generation.to_string(),"slots":slots}));
        }
        result["version_protection"] = json!(protection);
        let protection = if prepared.is_empty() {
            ProtectionAtPublish::NotApplicable
        } else {
            ProtectionAtPublish::TargetMet
        };
        let op = OperationResult {
            operation_id: operation_id.into(),
            kind: kind.into(),
            commit_id: result
                .get("commit_id")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            history_tree_id: result
                .get("history_tree_id")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            branch_id: result
                .get("branch_id")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            commit_state: storage_contract::CommitState::Published,
            protection_at_publish: protection,
            current_health: "HEALTHY".into(),
            request_digest: digest.clone(),
            sequence: next.sequence,
            result,
        };
        next.operations.insert(operation_id.into(), op.clone());
        next.operation_principals
            .insert(operation_id.into(), principal.into());
        next.events.push(Event {
            sequence: next.sequence,
            operation_id: operation_id.into(),
            kind: kind.into(),
            object_id: op
                .commit_id
                .clone()
                .or_else(|| op.history_tree_id.clone())
                .unwrap_or_else(|| operation_id.into()),
        });
        Ok(MutationPlan::Prepared {
            next: Box::new(next),
            prepared,
            result: op,
        })
    }
}
