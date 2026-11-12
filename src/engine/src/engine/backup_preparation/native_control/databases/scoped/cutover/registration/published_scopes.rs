//! Published SYSTEM and exact per-binding SQLite scopes. Only the selected
//! target's own current/history copies are installed at its held root. Source
//! roots and active SYSTEM/Current remain untouched until native activation.
use super::*;
use crate::disk::encode_metadata;
use crate::engine::backup_preparation::native_control::databases::materials::{
    self, Namespace, PreparedFiles,
};
use crate::engine::membership_transition::{AdmittedBackupDecision, BackupFirstPublicationHash};

pub(super) mod encoding_memo;
pub(super) mod projection;
use projection::Scopes;

const COMPLETE: &str = "published-scoped-databases-installed.json";
const MAX_COMPLETION: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BackupPublishedScopeTargetReport {
    pub target: DiskRegistrationIdentity,
    pub scope_manifest_hash: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub system_bytes: u64,
    pub configured_disks: usize,
    pub databases: usize,
    #[serialization(with = "storage_contract::u64_string")]
    pub scoped_bytes: u64,
    pub disk_current_hash: String,
    pub disk_history_hash: String,
}

/// Real database byte readback, not a permission to open a runtime target,
/// replace active SYSTEM, select an authority or release the original fence.
#[derive(Clone, Debug)]
pub struct BackupPublishedScopesReport {
    pub serving: BackupServingImagesReport,
    pub targets: Vec<BackupPublishedScopeTargetReport>,
}
#[derive(Serialize)]
struct Completion<'a> {
    format: &'static str,
    decision_hash: &'a str,
    publication_hash: &'a str,
    registry: &'a DiskRegistryCommitment,
    entities: &'a [BackupServingTargetReport],
    targets: &'a [BackupPublishedScopeTargetReport],
}
struct Plan {
    registry: published_registry::Plan,
    report: BackupPublishedScopesReport,
    completion: Vec<u8>,
}
impl Plan {
    fn capture(
        prepared: &PreparedBackupRegistry,
        decision: &AdmittedBackupDecision,
        expected: &BackupFirstPublicationHash,
        stop: &AtomicBool,
    ) -> Result<Self> {
        recovery::require_isolated(&prepared.targets)?;
        let serving = serving::observe(prepared, decision, expected, stop)?;
        let registry = published_registry::Plan::capture(prepared, decision, expected, stop)?;
        let mut plan = Self {
            registry,
            report: BackupPublishedScopesReport {
                serving,
                targets: Vec::with_capacity(prepared.targets.len()),
            },
            completion: Vec::new(),
        };
        for index in 0..prepared.targets.len() {
            let scopes = Scopes::capture(prepared, &plan.registry, index, stop)?;
            plan.report
                .targets
                .push(scopes.report(&prepared.projection.targets[index].node.identity));
        }
        let configuration = plan.registry.configuration();
        let publication = &configuration.publication.publication;
        plan.completion = encode_metadata(
            &Completion {
                format: "zxs-backup-published-scoped-databases-installed",
                decision_hash: publication.decision_hash.as_str(),
                publication_hash: &publication.publication_hash,
                registry: &configuration.registry,
                entities: &plan.report.serving.targets,
                targets: &plan.report.targets,
            },
            MAX_COMPLETION,
        )?;
        Ok(plan)
    }
    fn with_scopes<T>(
        &self,
        prepared: &PreparedBackupRegistry,
        index: usize,
        stop: &AtomicBool,
        action: impl FnOnce(&DiskRoot, &Scopes) -> Result<T>,
    ) -> Result<T> {
        // Reconstruct from this target, never a peer or an old source database.
        let scopes = Scopes::capture(prepared, &self.registry, index, stop)?;
        if scopes.report(&prepared.projection.targets[index].node.identity)
            != self.report.targets[index]
        {
            return Err(invalid(
                "published SQLite scopes changed after their original plan",
            ));
        }
        let result = action(&prepared.targets[index].root, &scopes)?;
        named_target(
            &prepared.targets[index].root,
            &prepared.task.selection().jobs()[index],
        )?;
        Ok(result)
    }
    fn completion(&self) -> materials::RootRecord<'_> {
        materials::RootRecord {
            name: COMPLETE,
            bytes: self.completion.as_slice().into(),
            maximum: MAX_COMPLETION,
        }
    }
    fn observe(
        &self,
        prepared: &PreparedBackupRegistry,
        decision: &AdmittedBackupDecision,
        expected: &BackupFirstPublicationHash,
        stop: &AtomicBool,
    ) -> Result<BackupPublishedScopesReport> {
        recovery::require_isolated(&prepared.targets)?;
        for index in 0..prepared.targets.len() {
            self.with_scopes(prepared, index, stop, |root, scopes| {
                verify(root, scopes, stop)?;
                self.completion().require(root)
            })?;
        }
        serving::observe(prepared, decision, expected, stop)?;
        prepared.recheck_names()?;
        Ok(self.report.clone())
    }
}

fn verify(root: &DiskRoot, scopes: &Scopes, stop: &AtomicBool) -> Result<()> {
    materials::verify(root, &scopes.prepared()?, stop)?;
    for (name, bytes) in scopes.own() {
        preparation_checkpoint(stop)?;
        if root.read_bytes(&root.path().join(name), bytes.len())? != bytes {
            return Err(invalid(
                "target root SQLite differs from its published physical binding",
            ));
        }
    }
    Ok(())
}
fn require_capacity(root: &DiskRoot, scopes: &Scopes, stop: &AtomicBool) -> Result<()> {
    // Aggregate private scopes AND the two root copies; checking each in
    // isolation would allow the same free bytes to satisfy both obligations.
    let mut needed = materials::required_capacity(root, &scopes.prepared()?, stop)?;
    for (name, bytes) in scopes.own() {
        preparation_checkpoint(stop)?;
        let path = root.path().join(name);
        if preparation_path_exists(root, &path)? {
            if root.read_bytes(&path, bytes.len())? != bytes {
                return Err(Error::new(
                    ErrorCode::ObjectCollision,
                    "existing root database has another scope or bytes and is preserved",
                ));
            }
        } else {
            needed = needed
                .checked_add(bytes.len() as u64)
                .ok_or_else(projection::budget)?;
        }
    }
    needed = needed
        .checked_add(16 * 1024)
        .ok_or_else(projection::budget)?;
    if root.available()? < needed {
        return Err(Error::new(
            ErrorCode::NoSpace,
            "target cannot hold its complete published scopes and independent root copies",
        ));
    }
    Ok(())
}

/// Conservative additional space before NEW irreversible native choice. Actual
/// missing-byte admission still runs on every root before any installation.
pub(super) const PRE_CHOICE_BYTES: u64 =
    projection::MAX_BYTES as u64 + 64 * 1024 * 1024 + 18 * 1024 * 1024;

pub(super) fn install(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupPublishedScopesReport> {
    let plan = Plan::capture(prepared, decision, expected, stop)?;
    // No target is written before every exact original target passes preflight.
    for index in 0..prepared.targets.len() {
        plan.with_scopes(prepared, index, stop, |root, scopes| {
            if preparation_path_exists(root, &root.path().join(COMPLETE))? {
                verify(root, scopes, stop)?;
                plan.completion().require(root)?;
            }
            require_capacity(root, scopes, stop)
        })?;
    }
    prepared.write_isolated_targets(stop, |index, _| {
        plan.with_scopes(prepared, index, stop, |root, scopes| {
            require_capacity(root, scopes, stop)?;
            materials::install(root, &scopes.prepared()?, stop)?;
            for (name, bytes) in scopes.own() {
                preparation_checkpoint(stop)?;
                // Never open SQLite by filesystem path or replace an existing
                // database. Matching interrupted files are retained and synced.
                install_preparation_bytes(root, &root.path().join(name), bytes)?;
            }
            verify(root, scopes, stop)
        })
    })?;
    for index in 0..prepared.targets.len() {
        plan.with_scopes(prepared, index, stop, |root, scopes| {
            verify(root, scopes, stop)
        })?;
    }
    serving::observe(prepared, decision, expected, stop)?;
    prepared.write_completions(&plan.completion(), stop)?;
    plan.observe(prepared, decision, expected, stop)
}
pub(super) fn observe(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupPublishedScopesReport> {
    Plan::capture(prepared, decision, expected, stop)?.observe(prepared, decision, expected, stop)
}
