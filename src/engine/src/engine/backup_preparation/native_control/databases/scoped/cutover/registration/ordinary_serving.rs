//! Complete ordinary target containers, exact object databases and native
//! locator readback under the retained real target writers. No source selector,
//! Current, next publication or fence release is installed by this stage.
use super::*;
use crate::container_storage::backup_ordinary::Image;
use crate::engine::membership_transition::{AdmittedBackupDecision, BackupFirstPublicationHash};

const INTENT: &str = "backup-preparation/ordinary-serving.json";
const SELECTED: &str = "backup-ordinary-serving-selected.json";
const LOCAL_COMPLETE: &str = "backup-ordinary-serving-installed.json";
const COMPLETE: &str = "backup-ordinary-serving-set-installed.json";
const MAX_RECORD: usize = 64 * 1024;
const MAX_LOCAL: usize = 32 * 1024 * 1024;

#[derive(Clone, Debug, Serialize)]
pub struct BackupOrdinaryServingTargetReport {
    pub identity: DiskRegistrationIdentity,
    pub recipe_hash: String,
    pub entities_hash: String,
    pub whole_files: usize,
    pub directory_repositories: usize,
    #[serialization(with = "storage_contract::u64_string")]
    pub available_versions: u64,
    #[serialization(with = "storage_contract::u64_string")]
    pub content_bytes: u64,
}

/// Actual ordinary-reader closure, not an active Engine or writable namespace.
#[derive(Clone, Debug)]
pub struct BackupOrdinaryServingReport {
    pub original: BackupOriginalControlReport,
    pub installation_hash: String,
    pub targets: Vec<BackupOrdinaryServingTargetReport>,
}

#[derive(Serialize)]
struct TargetRecipe {
    identity: DiskRegistrationIdentity,
    physical: PhysicalBinding,
    recipe_hash: String,
    whole_files: usize,
    directory_repositories: usize,
    #[serialization(with = "storage_contract::u64_string")]
    available_versions: u64,
    #[serialization(with = "storage_contract::u64_string")]
    content_bytes: u64,
}
struct Plan {
    original: BackupOriginalControlReport,
    registry: published_registry::Plan,
    targets: Vec<TargetRecipe>,
    bytes: Vec<u8>,
    hash: String,
}

impl Plan {
    fn capture(
        context: &common_databases::Context<'_>,
        prepared: &PreparedBackupRegistry,
        decision: &AdmittedBackupDecision,
        expected: &BackupFirstPublicationHash,
        stop: &AtomicBool,
    ) -> Result<Self> {
        let original = original_control::observe(context, prepared, decision, expected, stop)?;
        let registry = published_registry::Plan::capture(prepared, decision, expected, stop)?;
        let mut plan = Self {
            original,
            registry,
            targets: Vec::new(),
            bytes: Vec::new(),
            hash: String::new(),
        };
        for index in 0..prepared.targets.len() {
            let recipe = plan.with_image(prepared, index, stop, |image, root| {
                Ok(TargetRecipe {
                    identity: prepared.projection.targets[index].node.identity.clone(),
                    physical: PhysicalBinding::of(root)?,
                    recipe_hash: image.hash(),
                    whole_files: image.whole_files(),
                    directory_repositories: image.directory_repositories,
                    available_versions: image.bodies,
                    content_bytes: image.body_bytes,
                })
            })?;
            plan.targets.push(recipe);
        }
        #[derive(Serialize)]
        struct Manifest<'a> {
            format: &'static str,
            source_fence_hash: &'a str,
            decision_hash: &'a str,
            publication_hash: &'a str,
            original_control_hash: &'a str,
            targets: &'a [TargetRecipe],
        }
        plan.bytes = canonical_json(&Manifest {
            format: "zxs-backup-ordinary-serving",
            source_fence_hash: prepared.source_fence_hash(),
            decision_hash: decision.hash().as_str(),
            publication_hash: expected.as_str(),
            original_control_hash: &plan.original.installation_hash,
            targets: &plan.targets,
        })?;
        if plan.bytes.len() > MAX_RECORD {
            return Err(invalid("ordinary Backup plan exceeds 64 KiB"));
        }
        plan.hash = blake3::hash(&plan.bytes).to_hex().to_string();
        Ok(plan)
    }
    fn io_hash(&self) -> &str {
        &self.original.targets.seals.io.admission_hash
    }
    fn with_image<T>(
        &self,
        prepared: &PreparedBackupRegistry,
        index: usize,
        stop: &AtomicBool,
        action: impl FnOnce(&Image<'_>, &DiskRoot) -> Result<T>,
    ) -> Result<T> {
        let target = &prepared.targets[index];
        let node = &prepared.projection.targets[index].node;
        target.with_writer_io(self.io_hash(), |root| {
            let registry = self.registry.verify(target, stop)?;
            if registry.registration(&node.identity)? != Some(crate::RegistrationMode::Enabled) {
                return Err(invalid(
                    "ordinary Backup target is not in its exact published registry",
                ));
            }
            let state = self.registry.state_at(target, stop)?;
            let image = Image::capture(
                &state,
                &node.identity.slot_id,
                &node.source_prefix,
                &self
                    .registry
                    .configuration()
                    .publication
                    .publication
                    .publication_hash,
                stop,
            )?;
            let job = &prepared.task.selection().jobs()[index];
            if image.whole_files()
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
                    "ordinary Backup target omits part of an original whole unit",
                ));
            }
            if let Some(expected) = self.targets.get(index)
                && (image.hash() != expected.recipe_hash
                    || PhysicalBinding::of(root)? != expected.physical)
            {
                return Err(invalid(
                    "ordinary Backup target changed its exact recipe or physical root",
                ));
            }
            action(&image, root)
        })
    }
    fn selection(&self, index: usize) -> Result<Vec<u8>> {
        #[derive(Serialize)]
        struct Selection<'a> {
            format: &'static str,
            plan_hash: &'a str,
            target: &'a TargetRecipe,
        }
        let bytes = canonical_json(&Selection {
            format: "zxs-backup-ordinary-serving-selection",
            plan_hash: &self.hash,
            target: &self.targets[index],
        })?;
        if bytes.len() > 8192 {
            return Err(invalid("ordinary Backup local selection exceeds 8 KiB"));
        }
        Ok(bytes)
    }
    fn require_selection(&self, root: &DiskRoot, index: usize) -> Result<()> {
        if root.read_bytes(&root.path().join(SELECTED), 8192)? != self.selection(index)? {
            return Err(invalid(
                "ordinary Backup target lost its exact original selection",
            ));
        }
        Ok(())
    }
    fn preflight(
        &self,
        prepared: &PreparedBackupRegistry,
        index: usize,
        reserve: u64,
        stop: &AtomicBool,
    ) -> Result<()> {
        self.with_image(prepared, index, stop, |image, root| {
            if preparation_path_exists(root, &root.path().join(SELECTED))? {
                self.require_selection(root, index)?;
            } else {
                image.require_absent(root, stop)?;
                if preparation_path_exists(root, &root.path().join(LOCAL_COMPLETE))? {
                    return Err(invalid(
                        "ordinary Backup local completion lost its original selection",
                    ));
                }
            }
            if preparation_path_exists(root, &root.path().join(LOCAL_COMPLETE))? {
                self.require_local(root, image, stop)?;
                Ok(())
            } else {
                image.require_capacity(root, reserve, stop)
            }
        })
    }
    fn require_local(
        &self,
        root: &DiskRoot,
        image: &Image<'_>,
        stop: &AtomicBool,
    ) -> Result<String> {
        let observed = image.verify(root, stop)?;
        if root.read_bytes(&root.path().join(LOCAL_COMPLETE), MAX_LOCAL)? != observed {
            return Err(invalid(
                "completed ordinary Backup entity, body, database, locator or inode changed; no repair is permitted",
            ));
        }
        Ok(blake3::hash(&observed).to_hex().to_string())
    }
    fn verify_targets(
        &self,
        prepared: &PreparedBackupRegistry,
        stop: &AtomicBool,
    ) -> Result<Vec<BackupOrdinaryServingTargetReport>> {
        let mut reports = Vec::with_capacity(prepared.targets.len());
        for (index, recipe) in self.targets.iter().enumerate() {
            let hash = self.with_image(prepared, index, stop, |image, root| {
                self.require_selection(root, index)?;
                self.require_local(root, image, stop)
            })?;
            reports.push(BackupOrdinaryServingTargetReport {
                identity: recipe.identity.clone(),
                recipe_hash: recipe.recipe_hash.clone(),
                entities_hash: hash,
                whole_files: recipe.whole_files,
                directory_repositories: recipe.directory_repositories,
                available_versions: recipe.available_versions,
                content_bytes: recipe.content_bytes,
            });
        }
        Ok(reports)
    }
    fn require_intent(&self, system: &DiskRoot) -> Result<()> {
        if system.read_bytes(&system.path().join(INTENT), MAX_RECORD)? != self.bytes {
            return Err(invalid(
                "ordinary Backup SYSTEM intent changed its original complete plan",
            ));
        }
        Ok(())
    }
    fn completion(&self, reports: &[BackupOrdinaryServingTargetReport]) -> Result<Vec<u8>> {
        #[derive(Serialize)]
        struct Completion<'a> {
            format: &'static str,
            plan_hash: &'a str,
            targets: &'a [BackupOrdinaryServingTargetReport],
        }
        let bytes = canonical_json(&Completion {
            format: "zxs-backup-ordinary-serving-set",
            plan_hash: &self.hash,
            targets: reports,
        })?;
        if bytes.len() > MAX_RECORD {
            return Err(invalid("ordinary Backup completion exceeds 64 KiB"));
        }
        Ok(bytes)
    }
}

fn require_completion(root: &DiskRoot, bytes: &[u8]) -> Result<()> {
    if root.read_bytes(&root.path().join(COMPLETE), MAX_RECORD)? != bytes {
        return Err(invalid(
            "ordinary Backup common completion differs from every target's actual complete entities",
        ));
    }
    Ok(())
}

pub(super) fn install(
    context: &common_databases::Context<'_>,
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupOrdinaryServingReport> {
    let plan = Plan::capture(context, prepared, decision, expected, stop)?;
    let mut common = false;
    target_writers::visit(context, prepared, stop, |root| {
        common |= preparation_path_exists(root, &root.path().join(COMPLETE))?;
        Ok(())
    })?;
    if common || preparation_path_exists(context.system, &context.system.path().join(INTENT))? {
        plan.require_intent(context.system)?;
    }
    if common {
        plan.verify_targets(prepared, stop)?;
    } else {
        for index in 0..prepared.targets.len() {
            plan.preflight(
                prepared,
                index,
                context.sources.metadata_reserve_bytes(),
                stop,
            )?;
        }
        let minimum = context
            .sources
            .metadata_reserve_bytes()
            .checked_add(1024 * 1024)
            .ok_or_else(|| invalid("ordinary Backup completion reserve overflow"))?;
        target_writers::visit(context, prepared, stop, |root| {
            if root.available()? < minimum {
                return Err(Error::new(
                    ErrorCode::NoSpace,
                    "ordinary Backup completion needs reserve plus 1 MiB on each participant",
                ));
            }
            Ok(())
        })?;
        install_preparation_bytes(
            context.system,
            &context.system.path().join(INTENT),
            &plan.bytes,
        )?;
        // Every target's original selection is durable before the first new
        // ordinary entity. No source data or active alias is changed.
        for (index, target) in prepared.targets.iter().enumerate() {
            target.with_writer_io(plan.io_hash(), |root| {
                install_preparation_bytes(
                    root,
                    &root.path().join(SELECTED),
                    &plan.selection(index)?,
                )
            })?;
        }
        for index in 0..prepared.targets.len() {
            plan.with_image(prepared, index, stop, |image, root| {
                plan.require_selection(root, index)?;
                if preparation_path_exists(root, &root.path().join(LOCAL_COMPLETE))? {
                    plan.require_local(root, image, stop)?;
                } else {
                    image.require_capacity(root, context.sources.metadata_reserve_bytes(), stop)?;
                    image.install(root, stop)?;
                    let observed = image.verify(root, stop)?;
                    install_preparation_bytes(root, &root.path().join(LOCAL_COMPLETE), &observed)?;
                    plan.require_local(root, image, stop)?;
                }
                Ok(())
            })?;
        }
    }
    let before = plan.bytes.clone();
    drop(plan);
    let after = Plan::capture(context, prepared, decision, expected, stop)?;
    if after.bytes != before {
        return Err(invalid(
            "ordinary Backup inputs changed before common completion",
        ));
    }
    after.require_intent(context.system)?;
    let targets = after.verify_targets(prepared, stop)?;
    let completion = after.completion(&targets)?;
    // Validate all existing common records before filling only absent copies.
    target_writers::visit(context, prepared, stop, |root| {
        if preparation_path_exists(root, &root.path().join(COMPLETE))? {
            require_completion(root, &completion)?;
        }
        Ok(())
    })?;
    for target in &prepared.targets {
        target.with_writer_io(after.io_hash(), |root| {
            install_preparation_bytes(root, &root.path().join(COMPLETE), &completion)
        })?;
    }
    context.sources.for_each_disk(
        stop,
        |root, _| install_preparation_bytes(root, &root.path().join(COMPLETE), &completion),
        &prepared.task.selection().first().pool_id,
    )?;
    install_preparation_bytes(
        context.system,
        &context.system.path().join(COMPLETE),
        &completion,
    )?;
    target_writers::visit(context, prepared, stop, |root| {
        require_completion(root, &completion)
    })?;
    context.sources.recheck(context.system)?;
    prepared.recheck_names()?;
    Ok(BackupOrdinaryServingReport {
        original: after.original,
        installation_hash: after.hash,
        targets,
    })
}

pub(super) fn observe(
    context: &common_databases::Context<'_>,
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupOrdinaryServingReport> {
    let plan = Plan::capture(context, prepared, decision, expected, stop)?;
    plan.require_intent(context.system)?;
    let targets = plan.verify_targets(prepared, stop)?;
    let completion = plan.completion(&targets)?;
    target_writers::visit(context, prepared, stop, |root| {
        require_completion(root, &completion)
    })?;
    Ok(BackupOrdinaryServingReport {
        original: plan.original,
        installation_hash: plan.hash,
        targets,
    })
}
