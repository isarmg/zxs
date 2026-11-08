//! Published, whole-unit physical entities and their exact target locators.
//! No runtime member, Current writer or source-fence release is installed here.
use super::*;
use crate::container_storage::backup_entities::{Image, PublishedImageBinding};
use crate::engine::membership_transition::{AdmittedBackupDecision, BackupFirstPublicationHash};

mod reads;
pub(super) use reads::read_entry;
pub use reads::{BackupServingContent, BackupServingRead};

const COMPLETE: &str = "published-serving-images-installed.json";
const MAX_COMPLETION: usize = 64 * 1024;

/// All entity/SQLite/body copies on one exact target. This binds actual physical
/// addresses, not a runtime I/O capability or an independent recovery authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BackupServingTargetReport {
    pub target: DiskRegistrationIdentity,
    pub image_manifest_hash: String,
    pub whole_files: usize,
    pub directory_repositories: usize,
    #[serialization(with = "storage_contract::u64_string")]
    pub available_versions: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub content_bytes: u64,
}

#[derive(Clone, Debug)]
pub struct BackupServingImagesReport {
    pub registry: BackupPublishedRegistryReport,
    pub targets: Vec<BackupServingTargetReport>,
}

#[derive(Serialize)]
struct Completion<'a> {
    format: &'static str,
    decision_hash: &'a str,
    publication_hash: &'a str,
    registry: &'a DiskRegistryCommitment,
    targets: &'a [BackupServingTargetReport],
}

struct Plan {
    registry: published_registry::Plan,
    report: BackupServingImagesReport,
    completion: Vec<u8>,
}
impl Plan {
    fn capture(
        prepared: &PreparedBackupRegistry,
        decision: &AdmittedBackupDecision,
        expected: &BackupFirstPublicationHash,
        stop: &AtomicBool,
    ) -> Result<Self> {
        let registry = published_registry::Plan::capture(prepared, decision, expected, stop)?;
        let report = published_registry::observe_with_plan(prepared, decision, &registry, stop)?;
        let mut plan = Self {
            registry,
            report: BackupServingImagesReport {
                registry: report,
                targets: Vec::with_capacity(prepared.targets.len()),
            },
            completion: Vec::new(),
        };
        for index in 0..prepared.targets.len() {
            let target = plan.with_image(prepared, index, stop, |image, _| {
                Ok(plan.target_report(prepared, index, image))
            })?;
            plan.report.targets.push(target);
        }
        let publication = &plan.registry.configuration().publication.publication;
        plan.completion = canonical_json(&Completion {
            format: "zxs-backup-published-serving-images-installed",
            decision_hash: publication.decision_hash.as_str(),
            publication_hash: &publication.publication_hash,
            registry: &plan.registry.configuration().registry,
            targets: &plan.report.targets,
        })?;
        if plan.completion.len() > MAX_COMPLETION {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "published serving image completion exceeds 64 KiB",
            ));
        }
        Ok(plan)
    }

    fn with_image<T>(
        &self,
        prepared: &PreparedBackupRegistry,
        index: usize,
        stop: &AtomicBool,
        action: impl FnOnce(&Image<'_>, &State) -> Result<T>,
    ) -> Result<T> {
        let target = &prepared.targets[index];
        let node = &prepared.projection.targets[index].node;
        let job = &prepared.task.selection().jobs()[index];
        named_target(&target.root, job)?;
        // Never use target zero's decoded State, the source database, or the
        // old activation-image as this target's published serving recipe.
        let registry = self.registry.verify(target, stop)?;
        if registry.registration(&node.identity)? != Some(crate::RegistrationMode::Enabled) {
            return Err(invalid(
                "serving image target is not in its published registry",
            ));
        }
        let admitted = self.registry.state_at(target, stop)?;
        let configuration = self.registry.configuration();
        let publication = &configuration.publication.publication;
        let image = Image::capture_published(
            &target.root,
            &admitted,
            &node.source_prefix,
            PublishedImageBinding {
                source_fence_hash: prepared.source_fence_hash(),
                decision_hash: publication.decision_hash.as_str(),
                publication_hash: &publication.publication_hash,
                snapshot_hash: &publication.snapshot_hash,
                registry: registry.commitment(),
                target: &node.identity,
            },
            stop,
        )?;
        if image.file_units
            != prepared
                .task
                .installation
                .evidence
                .report
                .whole_file_units_per_target
            || image.directory_repositories
                != prepared
                    .task
                    .installation
                    .evidence
                    .report
                    .directory_repositories_per_target
            || image.bodies != job.total_files
            || image.body_bytes != job.total_bytes
        {
            return Err(invalid(
                "published serving image splits or omits the original whole units",
            ));
        }
        let result = action(&image, admitted.state())?;
        named_target(&target.root, job)?;
        Ok(result)
    }

    fn target_report(
        &self,
        prepared: &PreparedBackupRegistry,
        index: usize,
        image: &Image<'_>,
    ) -> BackupServingTargetReport {
        BackupServingTargetReport {
            target: prepared.projection.targets[index].node.identity.clone(),
            image_manifest_hash: image.manifest_hash(),
            whole_files: image.file_units,
            directory_repositories: image.directory_repositories,
            available_versions: image.bodies,
            content_bytes: image.body_bytes,
        }
    }

    fn require_completion(&self, target: &Target) -> Result<()> {
        if target
            .root
            .read_bytes(&target.root.path().join(COMPLETE), MAX_COMPLETION)?
            != self.completion
        {
            return Err(invalid(
                "serving image completion differs from the complete original set",
            ));
        }
        Ok(())
    }

    fn verify_target(
        &self,
        prepared: &PreparedBackupRegistry,
        index: usize,
        stop: &AtomicBool,
    ) -> Result<()> {
        self.with_image(prepared, index, stop, |image, _| {
            if self.target_report(prepared, index, image) != self.report.targets[index] {
                return Err(invalid(
                    "serving image no longer matches its exact published plan",
                ));
            }
            image.verify(&prepared.targets[index].root, stop)
        })
    }

    fn observe(
        &self,
        prepared: &PreparedBackupRegistry,
        decision: &AdmittedBackupDecision,
        expected: &BackupFirstPublicationHash,
        stop: &AtomicBool,
    ) -> Result<BackupServingImagesReport> {
        for (index, target) in prepared.targets.iter().enumerate() {
            self.verify_target(prepared, index, stop)?;
            self.require_completion(target)?;
        }
        first_publication::observe(prepared, decision, expected, stop)?;
        published_registry::observe_with_plan(prepared, decision, &self.registry, stop)?;
        Ok(self.report.clone())
    }
}

/// Called before selecting a NEW native decision. This conservative bound
/// avoids knowingly choosing a task whose independent serving copy cannot fit.
/// It does not invent a candidate, waive later actual capacity checks, or reserve
/// filesystem space against unrelated consumers.
pub(super) fn require_capacity_before_choice(
    prepared: &PreparedBackupRegistry,
    stop: &AtomicBool,
) -> Result<()> {
    let required = crate::container_storage::backup_entities::capacity_before_choice(
        prepared.task.selection().first().total_bytes,
    )?
    .checked_add(published_scopes::PRE_CHOICE_BYTES)
    .ok_or_else(|| {
        Error::new(
            ErrorCode::ResourceLimit,
            "complete published serving/scoped capacity overflow",
        )
    })?;
    for target in &prepared.targets {
        preparation_checkpoint(stop)?;
        if target.root.available()? < required {
            return Err(Error::new(
                ErrorCode::NoSpace,
                "selected target lacks room for complete published serving entities before the native choice",
            ));
        }
    }
    prepared.recheck_names()
}

/// All capacity and completed-image checks precede the first write. Interrupted
/// matching partial images may continue; an already completed damaged image
/// cannot be silently downgraded to partial and repaired from another copy.
pub(super) fn install(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupServingImagesReport> {
    let plan = Plan::capture(prepared, decision, expected, stop)?;
    for (index, target) in prepared.targets.iter().enumerate() {
        if preparation_path_exists(&target.root, &target.root.path().join(COMPLETE))? {
            plan.verify_target(prepared, index, stop)?;
            plan.require_completion(target)?;
        }
        plan.with_image(prepared, index, stop, |image, _| {
            image.require_capacity(&target.root, stop)
        })?;
    }
    for (index, target) in prepared.targets.iter().enumerate() {
        preparation_checkpoint(stop)?;
        recovery::require_isolated(&prepared.targets)?;
        prepared.recheck_names()?;
        plan.with_image(prepared, index, stop, |image, _| {
            image.install(&target.root, stop)
        })?;
    }
    for index in 0..prepared.targets.len() {
        plan.verify_target(prepared, index, stop)?;
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
        plan.require_completion(target)?;
    }
    plan.observe(prepared, decision, expected, stop)
}

pub(super) fn observe(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupServingImagesReport> {
    Plan::capture(prepared, decision, expected, stop)?.observe(prepared, decision, expected, stop)
}
