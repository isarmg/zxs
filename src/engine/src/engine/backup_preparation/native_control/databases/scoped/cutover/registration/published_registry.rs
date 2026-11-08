//! Target-local native registry installation after the actual first Publication.
//! Every page is independently retained and read back on every selected target.
//! This is a frozen namespace reader, never Current installation or writer I/O.
use super::*;
use crate::engine::backup_preparation::native_control::databases::materials::{
    self, Namespace, PreparedFiles,
};
use crate::engine::membership_transition::{AdmittedBackupDecision, BackupFirstPublicationHash};

mod namespaces;
pub(super) use namespaces::read_page;
pub use namespaces::{BackupNamespacePage, BackupNamespaceScope};

const DIRECTORY: &str = "backup-published-registry";
const COMPLETE: &str = "published-registry-installed.json";
const CONFIG: &str = "effective-config.json";
const MAX_MANIFEST: usize = 4 * 1024 * 1024;
const MAX_GRAPH_BYTES: usize = 64 * 1024 * 1024;
const MAX_GRAPH_OBJECTS: usize = 4096;

/// Actual native graph readback on the complete original target set. It does
/// not claim that an Engine has installed Current, admitted a target writer,
/// published a later namespace operation or released an original source fence.
#[derive(Clone, Debug)]
pub struct BackupPublishedRegistryReport {
    pub configuration: BackupConfigurationReport,
    pub metadata_bytes_per_target: u64,
    pub branch_scopes: usize,
    pub directory_scopes: usize,
}

#[derive(Serialize)]
struct FileReference {
    bytes: usize,
    digest: String,
}
#[derive(Serialize)]
struct Manifest<'a> {
    format: &'static str,
    source_fence_hash: &'a str,
    decision_hash: &'a str,
    publication_hash: &'a str,
    authority_hash: &'a str,
    snapshot_hash: &'a str,
    config_hash: &'a str,
    registry: &'a crate::DiskRegistryCommitment,
    targets: usize,
    files: BTreeMap<&'a PathBuf, FileReference>,
}
#[derive(Serialize)]
struct Completion<'a> {
    format: &'static str,
    decision_hash: &'a str,
    publication_hash: &'a str,
    registry: &'a crate::DiskRegistryCommitment,
    manifest_hash: &'a str,
    targets: usize,
}

struct Plan {
    resolved: configuration::ResolvedConfiguration,
    files: BTreeMap<PathBuf, Vec<u8>>,
    manifest: Vec<u8>,
    completion: Vec<u8>,
    bytes: u64,
}
fn bounded(value: &impl Serialize) -> Result<Vec<u8>> {
    let bytes = canonical_json(value)?;
    if bytes.len() > MAX_MANIFEST {
        return Err(budget());
    }
    Ok(bytes)
}
fn budget() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "Backup registry exceeds its bounded graph, descriptor or decode budget",
    )
}
fn object_path(kind: &str, hash: &str) -> Result<PathBuf> {
    if !matches!(
        kind,
        "disk-lifecycle-registry" | "metadata-index" | "state-page"
    ) || hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid(
            "Backup registry requested an unknown native object",
        ));
    }
    Ok(PathBuf::from("objects")
        .join(kind)
        .join(&hash[..2])
        .join(format!("{hash}.json")))
}

impl Plan {
    fn capture(
        prepared: &PreparedBackupRegistry,
        decision: &AdmittedBackupDecision,
        expected: &BackupFirstPublicationHash,
        stop: &AtomicBool,
    ) -> Result<Self> {
        let resolved = configuration::resolve(prepared, decision, expected, stop)?;
        let mut files = BTreeMap::new();
        let mut bytes = 0usize;
        for (kind, hash, value) in resolved.registry.objects() {
            preparation_checkpoint(stop)?;
            bytes = bytes.checked_add(value.len()).ok_or_else(budget)?;
            if files.len() == MAX_GRAPH_OBJECTS || bytes > MAX_GRAPH_BYTES {
                return Err(budget());
            }
            if files
                .insert(object_path(kind, hash)?, value.to_vec())
                .is_some()
            {
                return Err(invalid("Backup registry repeats an object name"));
            }
        }
        // These bytes are an exact recipe, never deserialize-and-admit Config.
        // New roots cannot inherit Genesis I/O by being present in this file.
        let config = bounded(&resolved.config)?;
        bytes = bytes.checked_add(config.len()).ok_or_else(budget)?;
        files.insert(PathBuf::from(CONFIG), config);
        let publication = &resolved.report.publication.publication;
        let manifest = bounded(&Manifest {
            format: "zxs-backup-published-registry",
            source_fence_hash: prepared.source_fence_hash(),
            decision_hash: publication.decision_hash.as_str(),
            publication_hash: &publication.publication_hash,
            authority_hash: &publication.authority_hash,
            snapshot_hash: &publication.snapshot_hash,
            config_hash: &resolved.report.config_hash,
            registry: resolved.registry.commitment(),
            targets: prepared.targets.len(),
            files: files
                .iter()
                .map(|(path, value)| {
                    (
                        path,
                        FileReference {
                            bytes: value.len(),
                            digest: blake3::hash(value).to_hex().to_string(),
                        },
                    )
                })
                .collect(),
        })?;
        let completion = bounded(&Completion {
            format: "zxs-backup-published-registry-installed",
            decision_hash: publication.decision_hash.as_str(),
            publication_hash: &publication.publication_hash,
            registry: resolved.registry.commitment(),
            manifest_hash: blake3::hash(&manifest).to_hex().as_str(),
            targets: prepared.targets.len(),
        })?;
        bytes = bytes
            .checked_add(manifest.len())
            .and_then(|bytes| bytes.checked_add(completion.len()))
            .ok_or_else(budget)?;
        Ok(Self {
            resolved,
            files,
            manifest,
            completion,
            bytes: bytes as u64,
        })
    }
    fn files(&self) -> Result<PreparedFiles<'_>> {
        PreparedFiles::new(
            Namespace::PublishedRegistry,
            self.manifest.clone(),
            self.files
                .iter()
                .map(|(path, bytes)| (path.clone(), bytes.as_slice())),
        )
    }
    fn preflight(&self, target: &Target, stop: &AtomicBool) -> Result<()> {
        // Once a target has claimed completion, missing/corrupt pages are a
        // fault, not permission to repair by synthesizing a new graph.
        if preparation_path_exists(&target.root, &target.root.path().join(COMPLETE))? {
            self.verify(target, stop)?;
            self.require_completion(&target.root)?;
        }
        materials::require_capacity(&target.root, &self.files()?, stop)
    }
    fn verify(&self, target: &Target, stop: &AtomicBool) -> Result<crate::VerifiedDiskRegistry> {
        materials::verify(&target.root, &self.files()?, stop)?;
        let held = target
            .root
            .private_child(&target.root.path().join(DIRECTORY))?;
        let mut count = 0usize;
        let mut bytes = 0usize;
        let registry = self.resolved.registry.verify_backup_with(|kind, hash| {
            preparation_checkpoint(stop)?;
            count += 1;
            if count > MAX_GRAPH_OBJECTS * 2 {
                return Err(budget());
            }
            let relative = object_path(kind, hash)?;
            let expected = self.files.get(&relative).ok_or_else(|| {
                invalid("Backup registry page is outside the exact selected graph")
            })?;
            bytes = bytes.checked_add(expected.len()).ok_or_else(budget)?;
            // Parent and successor share immutable pages. Every actual decode
            // counts, including repeats; it is not a deduplication exemption.
            if bytes > MAX_GRAPH_BYTES * 2 {
                return Err(budget());
            }
            held.read_bytes(&held.path().join(relative), expected.len())
        })?;
        registry.require_config(&self.resolved.config)?;
        Ok(registry)
    }
    fn require_completion(&self, root: &DiskRoot) -> Result<()> {
        if root.read_bytes(&root.path().join(COMPLETE), MAX_MANIFEST)? != self.completion {
            return Err(invalid(
                "Backup registry has no exact all-target completion",
            ));
        }
        Ok(())
    }
    fn state_at(
        &self,
        target: &Target,
        stop: &AtomicBool,
    ) -> Result<crate::history_package::AdmittedState> {
        self.resolved.candidate.read_state_at(
            &target
                .root
                .private_child(&target.root.path().join("backup-generation-first-candidate"))?,
            stop,
        )
    }
    fn report(&self, state: &State) -> BackupPublishedRegistryReport {
        BackupPublishedRegistryReport {
            configuration: self.resolved.report.clone(),
            metadata_bytes_per_target: self.bytes,
            branch_scopes: state.branches.len(),
            directory_scopes: state.directory_branches.len(),
        }
    }
}

/// This is called only after the real first Publication, under the same held
/// original task/target locks. It is also the exact explicit detached retry.
pub(super) fn install(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupPublishedRegistryReport> {
    let plan = Plan::capture(prepared, decision, expected, stop)?;
    for target in &prepared.targets {
        plan.preflight(target, stop)?;
    }
    for target in &prepared.targets {
        preparation_checkpoint(stop)?;
        recovery::require_isolated(&prepared.targets)?;
        prepared.recheck_names()?;
        materials::install(&target.root, &plan.files()?, stop)?;
    }
    for target in &prepared.targets {
        plan.verify(target, stop)?;
    }
    first_publication::observe(prepared, decision, expected, stop)?;
    for target in &prepared.targets {
        preparation_checkpoint(stop)?;
        recovery::require_isolated(&prepared.targets)?;
        prepared.recheck_names()?;
        install_preparation_bytes(
            &target.root,
            &target.root.path().join(COMPLETE),
            &plan.completion,
        )?;
        plan.require_completion(&target.root)?;
    }
    first_publication::observe(prepared, decision, expected, stop)?;
    observe_with_plan(prepared, decision, &plan, stop)
}

fn observe_with_plan(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    plan: &Plan,
    stop: &AtomicBool,
) -> Result<BackupPublishedRegistryReport> {
    for target in &prepared.targets {
        plan.verify(target, stop)?;
        plan.require_completion(&target.root)?;
    }
    let state = plan.state_at(&prepared.targets[0], stop)?;
    decision.verify_targets(prepared, stop)?;
    recovery::require_isolated(&prepared.targets)?;
    prepared.recheck_names()?;
    Ok(plan.report(state.state()))
}

pub(super) fn observe(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupPublishedRegistryReport> {
    let plan = Plan::capture(prepared, decision, expected, stop)?;
    observe_with_plan(prepared, decision, &plan, stop)
}
