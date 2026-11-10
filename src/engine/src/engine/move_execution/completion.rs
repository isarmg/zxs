//! One move completion binding, independent of file/folder graph formats.
//! It identifies the already frozen candidate; it is never a planner, receipt,
//! replacement State, or permission to publish a different operation.
use super::*;
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::linux::FileIdentity;

const ACTIVE: &str = "move-completion.json";
const COMPLETED: &str = "move-completion";

#[derive(Clone, Deserialize, Serialize)]
#[serialization(deny_unknown_fields)]
pub(in crate::engine) struct CompletionBinding {
    format: Format,
    pool_id: String,
    operation: OperationId,
    #[serialization(with = "storage_contract::u64_string")]
    sequence: u64,
    candidate_hash: String,
    snapshot_hash: String,
    previous_snapshot: Option<String>,
    request_digest: String,
    metadata_disk_ids: Vec<String>,
    config_hash: String,
}

#[derive(Clone, Deserialize, Serialize)]
enum Format {
    #[serialization(rename = "move-completion-v1")]
    MoveCompletion,
}

fn system_root(engine: &Engine) -> Result<DiskRoot> {
    let root = DiskRoot::open(&engine.root)?;
    let writer = root.open_regular(&root.path().join("writer.lock"))?;
    if FileIdentity::from_fd(&writer)? != engine._locks.system_identity()? {
        return Err(invalid("move completion SYSTEM writer identity changed"));
    }
    Ok(root)
}

fn read(root: &DiskRoot, path: &std::path::Path) -> Result<Option<CompletionBinding>> {
    if root.optional_metadata(path)?.is_none() {
        return Ok(None);
    }
    root.read_json_observed(path, &mut |_| Ok(())).map(Some)
}

impl CompletionBinding {
    pub(in crate::engine) fn new(engine: &Engine, candidate: &Candidate, hash: &str) -> Result<Self> {
        engine.verify_candidate_membership(candidate)?;
        if object_hash("candidate", candidate)? != hash {
            return Err(invalid("move completion does not name the frozen candidate"));
        }
        Ok(Self {
            format: Format::MoveCompletion,
            pool_id: candidate.pool_id.clone(),
            operation: candidate.operation_id,
            sequence: candidate.sequence,
            candidate_hash: hash.into(),
            snapshot_hash: candidate.snapshot_hash.clone(),
            previous_snapshot: candidate.previous_snapshot.clone(),
            request_digest: candidate.request_digest.clone(),
            metadata_disk_ids: candidate.metadata_disk_ids.clone(),
            config_hash: object_hash("move-completion-config", &engine.config)?,
        })
    }

    pub(in crate::engine) fn sequence(&self) -> u64 { self.sequence }

    pub(in crate::engine) fn candidate_hash(&self) -> &str { &self.candidate_hash }

    fn same(&self, other: &Self) -> Result<()> {
        if encode_metadata(self, MAX_METADATA)? != encode_metadata(other, MAX_METADATA)? {
            return Err(invalid("move completion binding belongs to another original candidate"));
        }
        Ok(())
    }

    fn archive(&self, root: &DiskRoot) -> std::path::PathBuf {
        root.path().join(COMPLETED).join(format!("{}.json", self.operation))
    }

    pub(in crate::engine) fn require_candidate(&self, candidate: &Candidate, hash: &str) -> Result<()> {
        if candidate.pool_id != self.pool_id
            || candidate.operation_id != self.operation
            || candidate.sequence != self.sequence
            || hash != self.candidate_hash
            || candidate.snapshot_hash != self.snapshot_hash
            || candidate.previous_snapshot != self.previous_snapshot
            || candidate.request_digest != self.request_digest
            || candidate.metadata_disk_ids != self.metadata_disk_ids
            || object_hash("candidate", candidate)? != self.candidate_hash
        {
            return Err(invalid("move completion cannot substitute its selected candidate"));
        }
        Ok(())
    }

    pub(in crate::engine) fn require_config(&self, engine: &Engine) -> Result<()> {
        if self.pool_id != engine.config.pool_id
            || self.config_hash != object_hash("move-completion-config", &engine.config)?
            || self.metadata_disk_ids != engine.members().iter().map(|disk| disk.disk_id.clone()).collect::<Vec<_>>()
        {
            return Err(invalid("move completion original member configuration changed"));
        }
        Ok(())
    }

    fn retain_at(&self, root: &DiskRoot) -> Result<()> {
        let active = root.path().join(ACTIVE);
        match (read(root, &active)?, read(root, &self.archive(root))?) {
            (Some(original), None) | (None, Some(original)) => self.same(&original),
            (None, None) => root.write_json(&active, self, false),
            (Some(_), Some(_)) => Err(invalid("move completion has both active and retired names")),
        }
    }

    /// SYSTEM is retained first so a partial replica preparation still fences
    /// the original operation. Missing replicas receive only these exact bytes.
    pub(in crate::engine) fn retain(&self, engine: &Engine) -> Result<()> {
        self.require_config(engine)?;
        self.retain_at(&system_root(engine)?)?;
        for disk in engine.members() {
            let owner = engine.disk_resources.get(&disk)?;
            let lease = owner.root.admit_io()?;
            let seal = validate_seal_observed(&engine.config, &disk, &mut |_| lease.check_running())?;
            validate_stored_seal(&owner.seal, &seal)?;
            self.retain_at(&owner.root)?;
            lease.check_running()?;
        }
        Ok(())
    }

    /// Startup discovers only the active original selector. It never scans
    /// completed directories for a convenient operation or current path.
    pub(in crate::engine) fn discover(engine: &Engine) -> Result<Option<Self>> {
        let system = system_root(engine)?;
        let mut selected = read(&system, &system.path().join(ACTIVE))?;
        for disk in engine.members() {
            let Some(owner) = engine.disk_resources.available(&disk) else { continue; };
            if let Some(observed) = read(&owner.root, &owner.root.path().join(ACTIVE))? {
                if let Some(original) = &selected {
                    original.same(&observed)?;
                } else {
                    selected = Some(observed);
                }
            }
        }
        if let Some(selected) = &selected { selected.require_config(engine)?; }
        Ok(selected)
    }

    fn retire_at(&self, root: &DiskRoot) -> Result<()> {
        let active = root.path().join(ACTIVE);
        let archived = self.archive(root);
        root.private_dir(&root.path().join(COMPLETED))?;
        match (read(root, &active)?, read(root, &archived)?) {
            (Some(original), None) => {
                self.same(&original)?;
                root.rename_noreplace_destination_first_synced(&active, &archived)?;
                self.same(&root.read_json_observed(&archived, &mut |_| Ok(()))?)
            }
            (None, Some(original)) => {
                self.same(&original)?;
                root.sync_dir(&root.path().join(COMPLETED))?;
                root.sync_dir(root.path())
            }
            (Some(_), Some(_)) => Err(invalid("move completion retirement has conflicting names")),
            (None, None) => Err(invalid("move completion lost the original selector")),
        }
    }

    /// Called only by finish_move after FILE, both disk databases, SYSTEM and
    /// all member history roots have been observed. SYSTEM is retired last.
    pub(in crate::engine) fn retire<O: MoveOutcome>(&self, engine: &Engine, published: &PublishedMove<O>) -> Result<()> {
        self.require_candidate(&published.candidate, &published.hash)?;
        self.require_config(engine)?;
        for disk in engine.members() {
            let owner = engine.disk_resources.get(&disk)?;
            let lease = owner.root.admit_io()?;
            self.retire_at(&owner.root)?;
            lease.check_running()?;
        }
        self.retire_at(&system_root(engine)?)
    }

    /// A historical original publication may advance the replayed namespace
    /// only after the common completion barrier was retired successfully.
    pub(in crate::engine) fn require_completed(engine: &Engine, candidate: &Candidate, hash: &str) -> Result<()> {
        let root = system_root(engine)?;
        let path = root.path().join(COMPLETED).join(format!("{}.json", candidate.operation_id));
        if let Some(original) = read(&root, &path)? {
            return original.require_candidate(candidate, hash);
        }
        // A reconstructed SYSTEM has no former private archive. Read only this
        // journal-selected operation on its surviving original metadata owners;
        // never enumerate archives or treat an arbitrary saved State as success.
        let mut original: Option<Self> = None;
        for disk in engine.members().into_iter()
            .filter(|disk| candidate.metadata_disk_ids.contains(&disk.disk_id))
        {
            let Some(owner) = engine.disk_resources.available(&disk) else { continue; };
            if let Some(active) = read(&owner.root, &owner.root.path().join(ACTIVE))? {
                if active.operation == candidate.operation_id {
                    return Err(invalid("original move still has an active completion binding"));
                }
            }
            let path = owner.root.path().join(COMPLETED).join(format!("{}.json", candidate.operation_id));
            if let Some(retired) = read(&owner.root, &path)? {
                retired.require_candidate(candidate, hash)?;
                if let Some(previous) = &original { previous.same(&retired)?; }
                else { original = Some(retired); }
            }
        }
        original.ok_or_else(|| invalid("published move still lacks its completed serving-scope barrier"))?
            .require_candidate(candidate, hash)
    }
}
