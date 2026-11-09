//! Per-original object stores, inside the single final native switch. A local
//! completion is only an exact comparison target, never permission to repair.
use super::*;
use crate::{container_storage::backup_original_objects, database_install::InstallAdmission};

const COMPLETE: &str = "backup-serving-original-objects.json";
const MAX_COMPLETE: usize = 4 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Recipe {
    identity: DiskRegistrationIdentity,
    root: PhysicalBinding,
    recipe_hash: String,
}

pub(super) struct Objects {
    source: crate::history_package::AdmittedState,
    pub(super) recipes: Vec<Recipe>,
}

impl Objects {
    pub(super) fn capture(
        prepared: &PreparedBackupRegistry,
        registry: &published_registry::Plan,
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
                let published = registry
                    .candidate()
                    .read_state_at(&root.private_child(&root.path().join("metadata"))?, stop)?;
                let plan = backup_original_objects::Plan::capture(
                    root,
                    &identity.slot_id,
                    &source,
                    &published,
                    stop,
                )?;
                recipes.push(Recipe {
                    identity: identity.clone(),
                    root: PhysicalBinding::of(root)?,
                    recipe_hash: plan.recipe_hash().into(),
                });
                Ok(())
            },
            &prepared.task.selection().first().pool_id,
        )?;
        Ok(Self { source, recipes })
    }

    fn plan(
        &self,
        registry: &published_registry::Plan,
        root: &DiskRoot,
        identity: &DiskRegistrationIdentity,
        stop: &AtomicBool,
    ) -> Result<backup_original_objects::Plan> {
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
        let published = registry
            .candidate()
            .read_state_at(&root.private_child(&root.path().join("metadata"))?, stop)?;
        let plan = backup_original_objects::Plan::capture(
            root,
            &identity.slot_id,
            &self.source,
            &published,
            stop,
        )?;
        if plan.recipe_hash() != expected.recipe_hash {
            return Err(invalid(
                "original Backup object recipe changed after common selection",
            ));
        }
        Ok(plan)
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
                let plan = self.plan(registry, root, identity, stop)?;
                // This always follows the actual native first Publication. Its
                // retained recovery budget must not be counted as a new operation.
                plan.require_capacity(InstallAdmission::DecidedRecovery, reserve, stop)
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
            let plan = self.plan(registry, root, identity, stop)?;
            let record = root.open_optional_read_regular(&root.path().join(COMPLETE))?;
            let actual = match record {
                Some(record) => {
                    let saved = read_file_bytes(record, MAX_COMPLETE)?;
                    let actual = plan.verify(stop)?;
                    if saved != actual.bytes() {
                        return Err(invalid("completed original Backup objects lost their exact inodes or bytes; no repair"));
                    }
                    actual
                }
                None => {
                    let actual = plan.apply_preserving(stop)?;
                    install_preparation_bytes(root, &root.path().join(COMPLETE), actual.bytes())?;
                    let check = self.plan(registry, root, identity, stop)?.verify(stop)?;
                    require_bytes(root, COMPLETE, check.bytes(), MAX_COMPLETE)?;
                    check
                }
            };
            completed.push(blake3::hash(actual.bytes()).to_hex().to_string());
            Ok(())
        }, pool)?;
        Ok(completed)
    }
}

pub(super) fn verify_recovered(
    roots: &[(DiskConfig, DiskRoot)],
    first: &crate::engine::membership_transition::PreparedBackupFirstCandidate,
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
        let published =
            first.read_state_at(&root.private_child(&root.path().join("metadata"))?, stop)?;
        let plan =
            backup_original_objects::Plan::capture(root, &disk.slot_id, source, &published, stop)?;
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
