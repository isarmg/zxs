//! Per-original object stores, inside the single final native switch. A local
//! completion is only an exact comparison target, never permission to repair.
use super::*;
use crate::{container_storage::BackupOriginalImage, database_install::InstallAdmission};
use crate::engine::membership_transition::PublishedBackupOriginal;

const COMPLETE: &str = "backup-serving-original-objects.json";
const MAX_COMPLETE: usize = 4 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Recipe {
    identity: DiskRegistrationIdentity,
    root: PhysicalBinding,
    recipe_hash: String,
}

pub(super) struct Objects<'a> {
    publication: &'a PreparedBackupFirstPublication,
    source: crate::history_package::AdmittedState,
    pub(super) recipes: Vec<Recipe>,
}

impl<'a> Objects<'a> {
    pub(super) fn capture(
        prepared: &PreparedBackupRegistry,
        registry: &published_registry::Plan,
        publication: &'a PreparedBackupFirstPublication,
        sources: &common_databases::Sources,
        stop: &AtomicBool,
    ) -> Result<Self> {
        let source = snapshot::read(
            &prepared.targets[0].root,
            prepared.task.selection().first(),
            stop,
        )?;
        if object_hash("backup-prepared-state", source.state())?
            != prepared.source_evidence().state_hash()
        {
            return Err(invalid(
                "Backup switch lost its native original object State",
            ));
        }
        let mut recipes = Vec::new();
        sources.for_each_disk(
            stop,
            |root, identity| {
                let metadata = root.private_child(&root.path().join(
                    crate::persist::layout::BACKUP_NATIVE_METADATA.as_path(),
                ))?;
                let published = registry.candidate().read_state_at(&metadata, stop)?;
                let scope = PublishedBackupOriginal::observe(
                    root, identity, registry.candidate(), publication, &source, &published, stop,
                )?;
                let plan = BackupOriginalImage::capture(&scope, stop)?;
                recipes.push(Recipe {
                    identity: identity.clone(),
                    root: PhysicalBinding::of(root)?,
                    recipe_hash: plan.recipe_hash().into(),
                });
                Ok(())
            },
            &prepared.task.selection().first().pool_id,
        )?;
        Ok(Self { publication, source, recipes })
    }

    fn with_plan<T>(
        &self,
        registry: &published_registry::Plan,
        root: &DiskRoot,
        identity: &DiskRegistrationIdentity,
        stop: &AtomicBool,
        action: impl FnOnce(BackupOriginalImage<'_>) -> Result<T>,
    ) -> Result<T> {
        let expected = self
            .recipes
            .iter()
            .find(|recipe| &recipe.identity == identity)
            .ok_or_else(|| {
                invalid("original object store is outside the complete Backup switch")
            })?;
        if PhysicalBinding::of(root)? != expected.root {
            return Err(invalid(
                "original Backup object store changed its actual held root",
            ));
        }
        let metadata = root.private_child(&root.path().join(
            crate::persist::layout::BACKUP_NATIVE_METADATA.as_path(),
        ))?;
        let published = registry.candidate().read_state_at(&metadata, stop)?;
        let scope = PublishedBackupOriginal::observe(
            root, identity, registry.candidate(), self.publication, &self.source, &published, stop,
        )?;
        let plan = BackupOriginalImage::capture(&scope, stop)?;
        if plan.recipe_hash() != expected.recipe_hash {
            return Err(invalid(
                "original Backup object recipe changed after common selection",
            ));
        }
        action(plan)
    }

    pub(super) fn require_capacity(
        &self,
        registry: &published_registry::Plan,
        sources: &common_databases::Sources,
        pool: &str,
        reserve: u64,
        stop: &AtomicBool,
    ) -> Result<()> {
        sources.for_each_disk(
            stop,
            |root, identity| {
                // This follows the actual native first Publication. The
                // retained recovery budget is not a fresh operation budget.
                self.with_plan(registry, root, identity, stop, |plan| {
                    plan.require_capacity(InstallAdmission::DecidedRecovery, reserve, stop)
                })
            },
            pool,
        )
    }

    pub(super) fn install(
        &self,
        registry: &published_registry::Plan,
        sources: &common_databases::Sources,
        pool: &str,
        common: &[u8],
        stop: &AtomicBool,
    ) -> Result<Vec<String>> {
        let mut completed = Vec::with_capacity(self.recipes.len());
        sources.for_each_disk(stop, |root, identity| {
            require_bytes(root, SELECTED, common, MAX_PLAN)?;
            let actual = self.with_plan(registry, root, identity, stop, |plan| {
                let record = root.open_optional_read_regular(&root.path().join(COMPLETE))?;
                match record {
                    Some(record) => {
                        let saved = read_file_bytes(record, MAX_COMPLETE)?;
                        let actual = plan.verify(stop)?;
                        if saved != actual.bytes() {
                            return Err(invalid(
                                "completed original Backup objects changed bytes or inodes",
                            ));
                        }
                        Ok(actual)
                    }
                    None => {
                        let actual = plan.apply_preserving(stop)?;
                        install_preparation_bytes(
                            root, &root.path().join(COMPLETE), actual.bytes(),
                        )?;
                        let check = self.with_plan(registry, root, identity, stop, |plan| {
                            plan.verify(stop)
                        })?;
                        require_bytes(root, COMPLETE, check.bytes(), MAX_COMPLETE)?;
                        Ok(check)
                    }
                }
            })?;
            completed.push(blake3::hash(actual.bytes()).to_hex().to_string());
            Ok(())
        }, pool)?;
        Ok(completed)
    }
}

pub(super) fn verify_recovered(
    roots: &[(DiskConfig, DiskRoot)],
    first: &crate::engine::membership_transition::PreparedBackupFirstCandidate,
    publication: &PreparedBackupFirstPublication,
    source: &crate::history_package::AdmittedState,
    recipes: &[Recipe],
    stop: &AtomicBool,
) -> Result<Vec<String>> {
    if recipes.len() != first.original_config().disks.len() {
        return Err(invalid(
            "Backup final selection omits original object recipes",
        ));
    }
    let mut completed = Vec::with_capacity(recipes.len());
    for (disk, recipe) in first.original_config().disks.iter().zip(recipes) {
        let (_, root) = roots
            .iter()
            .find(|(actual, _)| actual.disk_id == disk.disk_id)
            .ok_or_else(|| invalid("Backup resume lacks its original object root"))?;
        if recipe.identity
            != (DiskRegistrationIdentity {
                pool_id: first.original_config().pool_id.clone(),
                disk_id: disk.disk_id.clone(),
                slot_id: disk.slot_id.clone(),
                binding_generation: disk.binding_generation,
            })
            || recipe.root != PhysicalBinding::of(root)?
        {
            return Err(invalid(
                "Backup resume object recipe changes its native original root",
            ));
        }
        let metadata = root.private_child(&root.path().join(
            crate::persist::layout::BACKUP_NATIVE_METADATA.as_path(),
        ))?;
        let published = first.read_state_at(&metadata, stop)?;
        let scope = PublishedBackupOriginal::observe(
            root, &recipe.identity, first, publication, source, &published, stop,
        )?;
        let plan = BackupOriginalImage::capture(&scope, stop)?;
        if plan.recipe_hash() != recipe.recipe_hash {
            return Err(invalid(
                "Backup resume original object recipe changed after its selection",
            ));
        }
        // A bootstrap plan is selected only after ALL these completed. This
        // recovery path never applies a missing or damaged original object DB.
        let actual = plan.verify(stop)?;
        require_bytes(root, COMPLETE, actual.bytes(), MAX_COMPLETE)?;
        completed.push(blake3::hash(actual.bytes()).to_hex().to_string());
    }
    Ok(completed)
}

#[cfg(test)]
#[path = "../../../../../../../../../../../test/engine/unit/backup_original_published_tests.rs"]
mod tests;
