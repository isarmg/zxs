//! Target-local native registry installation after the actual first Publication.
//! Every page is independently retained and read back on every selected target.
//! This is a frozen namespace reader, never Current installation or writer I/O.
use super::*;
use crate::disk::encode_metadata;
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

pub(super) struct Plan {
    resolved: configuration::ResolvedConfiguration,
    files: BTreeMap<PathBuf, Vec<u8>>,
    manifest: Vec<u8>,
    completion: Vec<u8>,
    bytes: u64,
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
    pub(super) fn capture(
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
        let config = encode_metadata(&resolved.config, MAX_MANIFEST)?;
        bytes = bytes.checked_add(config.len()).ok_or_else(budget)?;
        files.insert(PathBuf::from(CONFIG), config);
        let publication = &resolved.report.publication.publication;
        let manifest = encode_metadata(
            &Manifest {
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
            },
            MAX_MANIFEST,
        )?;
        let completion = encode_metadata(
            &Completion {
                format: "zxs-backup-published-registry-installed",
                decision_hash: publication.decision_hash.as_str(),
                publication_hash: &publication.publication_hash,
                registry: resolved.registry.commitment(),
                manifest_hash: blake3::hash(&manifest).to_hex().as_str(),
                targets: prepared.targets.len(),
            },
            MAX_MANIFEST,
        )?;
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
    pub(super) fn configuration(&self) -> &BackupConfigurationReport {
        &self.resolved.report
    }
    pub(super) fn candidate(
        &self,
    ) -> &crate::engine::membership_transition::PreparedBackupFirstCandidate {
        &self.resolved.candidate
    }
    pub(super) fn effective_config(&self) -> &crate::ValidatedConfig {
        &self.resolved.config
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
            self.completion().require(&target.root)?;
        }
        materials::require_capacity(&target.root, &self.files()?, stop)
    }
    pub(super) fn verify(
        &self,
        target: &Target,
        stop: &AtomicBool,
    ) -> Result<crate::VerifiedDiskRegistry> {
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
    fn completion(&self) -> materials::RootRecord<'_> {
        materials::RootRecord {
            name: COMPLETE,
            bytes: &self.completion,
            maximum: MAX_MANIFEST,
        }
    }
    pub(super) fn state_at(
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
    /// The callback only receives a read-only typed Current loan built from
    /// this target's native Publication, graph and complete snapshot. None can
    /// be replaced with target zero's State or a cached public report.
    pub(super) fn with_namespaces<T>(
        &self,
        prepared: &PreparedBackupRegistry,
        index: usize,
        expected: &BackupFirstPublicationHash,
        stop: &AtomicBool,
        action: impl FnOnce(
            &crate::engine::disk_registry::current::backup::BackupNamespaceAdmission<'_>,
        ) -> Result<T>,
    ) -> Result<T> {
        let target = &prepared.targets[index];
        let job = &prepared.task.selection().jobs()[index];
        named_target(&target.root, job)?;
        let graph = self.verify(target, stop)?;
        let published = self.state_at(target, stop)?;
        let publication =
            first_publication::read_at(prepared, self.candidate(), expected, target, stop)?;
        with_archive_namespaces(
            prepared,
            index,
            NamespaceInputs {
                candidate: self.candidate(),
                registry: &graph,
                published: &published,
                publication: &publication,
            },
            stop,
            action,
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

/// Both isolated and ordinary immutable stores feed the same full native
/// namespace replay. Neither a store location nor a descriptor grants writes.
pub(super) struct NamespaceInputs<'a> {
    pub(super) candidate: &'a crate::engine::membership_transition::PreparedBackupFirstCandidate,
    pub(super) registry: &'a crate::VerifiedDiskRegistry,
    pub(super) published: &'a crate::history_package::AdmittedState,
    pub(super) publication:
        &'a crate::engine::membership_transition::PreparedBackupFirstPublication,
}

pub(super) fn with_archive_namespaces<T>(
    prepared: &PreparedBackupRegistry,
    index: usize,
    inputs: NamespaceInputs<'_>,
    stop: &AtomicBool,
    action: impl FnOnce(
        &crate::engine::disk_registry::current::backup::BackupNamespaceAdmission<'_>,
    ) -> Result<T>,
) -> Result<T> {
    let replay = archive_replay(prepared, index, inputs.candidate, stop)?;
    let view = replay.finish(
        inputs.publication,
        inputs.candidate,
        inputs.registry,
        inputs.published,
    )?;
    let result = action(&view)?;
    preparation_checkpoint(stop)?;
    named_target(
        &prepared.targets[index].root,
        &prepared.task.selection().jobs()[index],
    )?;
    Ok(result)
}

pub(super) fn archive_replay(
    prepared: &PreparedBackupRegistry,
    index: usize,
    candidate: &crate::engine::membership_transition::PreparedBackupFirstCandidate,
    stop: &AtomicBool,
) -> Result<crate::engine::disk_registry::current::backup::BackupNamespaceReplay> {
    let target = &prepared.targets[index];
    let job = &prepared.task.selection().jobs()[index];
    named_target(&target.root, job)?;
    let mut replay =
        crate::engine::disk_registry::current::backup::BackupNamespaceReplay::new(candidate)?;
    let (control, _) = crate::engine::backup_preparation::native_control::archive::read_observed(
        &target.root,
        job,
        &prepared
            .task
            .installation
            .evidence
            .task
            .scoped
            .objects
            .native
            .archive_hash,
        stop,
        &mut |_, candidate, _, state| replay.observe(candidate, state),
    )?;
    if control.state_hash != prepared.source_evidence().state_hash()
        || candidate.candidate().previous_snapshot.as_deref()
            != Some(control.snapshot_hash.as_str())
    {
        return Err(invalid(
            "Backup Current archive does not bind the original complete source",
        ));
    }
    Ok(replay)
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
        plan.completion().install(&target.root)?;
        plan.completion().require(&target.root)?;
    }
    first_publication::observe(prepared, decision, expected, stop)?;
    observe_with_plan(prepared, decision, &plan, stop)
}

pub(super) fn observe_with_plan(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    plan: &Plan,
    stop: &AtomicBool,
) -> Result<BackupPublishedRegistryReport> {
    for target in &prepared.targets {
        plan.verify(target, stop)?;
        plan.completion().require(&target.root)?;
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
