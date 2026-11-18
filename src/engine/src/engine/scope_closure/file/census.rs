//! Closed positive case: all registered FILEs on this holder are current,
//! ordinary, originally born here, and each has a complete clean body set.
//! Missing or unsupported occurrences fail the entire FILE section. A partial
//! read can never represent the collection, even when its route map is empty.
use super::{ResidentFileRows, clean_bodies::CleanFileBodies};
use crate::{DiskConfig, Result, ValidatedConfig, disk::DiskHandle, engine::{PlanningContext, fresh_layout}};
use crate::engine::scope_closure::{OrdinalBuilder, ScopeClosureError as Failure, directory::OrdinaryDirectories,
    inventory::row::{InventoryRow, Lifecycle, Location, Ownership}, schema::Material};
use data_serialization::Serialize;
use name_policy::LookupName;
use std::{collections::BTreeMap, sync::Arc};
use storage_contract::{DiskId, FileId, OperationId, working_tree::{DirectoryId, DiskRegistrationIdentity, LocalEntityIdentity}};

const LIMIT: usize = 65_536;
const PACKAGE: usize = 64 * 1024 * 1024;
fn reject() -> crate::Error { Failure::PublicationMismatch.into() }
fn require(ok: bool) -> Result<()> { if ok { Ok(()) } else { Err(reject()) } }

#[derive(Serialize)]
#[serialization(tag = "kind", rename_all = "snake_case")]
enum Object<'a> { FileContainer { file_id: FileId, material: &'a Material<FileId> } }

struct Occurrence {
    rows: ResidentFileRows, bodies: CleanFileBodies, material: Material<FileId>,
    parent: DirectoryId, name: LookupName, entity: LocalEntityIdentity, creation: OperationId,
}
pub(in crate::engine) struct ResidentFiles {
    disk: DiskConfig, registration: DiskRegistrationIdentity, root: Arc<DiskHandle>, files: BTreeMap<FileId, Occurrence>,
}
impl ResidentFiles {
    pub(in crate::engine) fn original(ctx: &PlanningContext<'_>, disk: &DiskConfig,
        parents: &OrdinaryDirectories) -> Result<Self> {
        let id: DiskId = disk.disk_id.parse()?;
        let binding = ctx.config.storage_roots.get(&id).ok_or_else(reject)?;
        let registration = fresh_layout::registration(ctx.config, disk);
        require(binding.registration == registration && ctx.config.disks.iter().any(|d| d == disk))?;
        let root = Arc::clone(ctx.disk_resources.get(disk)?);
        fresh_layout::verify_root(ctx.config, disk, &root.root)?;
        parents.require_owner(&root, &registration)?;
        parents.require(ctx.config)?;
        // Routes are supplementary obligations, NOT the FILE universe: a
        // metadata-only registration or an Unborn cannot disappear because
        // no version includes this slot in its payload route.
        for (key, place) in ctx.state.placements.iter() {
            if !place.route_slot_ids.iter().any(|slot| slot == &registration.slot_id) { continue; }
            let version = ctx.state.versions.get(key).ok_or_else(reject)?;
            let record = ctx.state.file_containers.get(&version.file_id.parse::<FileId>()?).ok_or_else(reject)?;
            require(place.version_id == *key && version.version_id == *key
                && place.binding_generations.get(&registration.slot_id) == Some(&registration.binding_generation)
                && record.replicas.contains_key(&id) && record.history_tree_id == version.history_tree_id.parse::<storage_contract::HistoryTreeId>()?)?;
        }
        let projection = crate::state_projection::ProjectionSnapshot::new(ctx.state);
        let mut total = 0usize;
        let mut files = BTreeMap::new();
        for (&file, record) in ctx.state.file_containers.iter() {
            let Some(occurrence) = record.replicas.get(&id) else { continue };
            if files.len() >= LIMIT { return Err(Failure::Budget.into()); }
            let created = ctx.state.operations.get(&record.created_by).ok_or_else(reject)?;
            require(created.operation_id == record.created_by && created.sequence != 0
                && created.sequence <= ctx.state.sequence && !ctx.state.file_deletions.contains_key(&file.to_string()))?;
            let parent = parents.file_parent(ctx,record)?;
            let rows = ResidentFileRows::current(ctx,disk,file)?;
            require(rows.registration() == &registration && rows.replica() == occurrence
                && Arc::ptr_eq(rows.holder.disk(), &root))?;
            let entity = LocalEntityIdentity::new(occurrence.entity.device(), occurrence.entity.inode());
            parents.require_file_entry(parent, &record.namespace.name, &rows.holder, entity)?;
            let bodies = rows.clean_full_bodies(ctx, projection.view())?;
            let material = bodies.material(&rows)?;
            total = total.checked_add(rows.image.bytes().len()).and_then(|n| n.checked_add(bodies.pages().bytes()))
                .filter(|n| *n <= PACKAGE).ok_or(Failure::Budget)?;
            files.insert(file, Occurrence { rows, bodies, material, parent, name: record.namespace.name.clone(),
                entity, creation: record.created_by });
        }
        let result = Self { disk: disk.clone(), registration, root, files };
        result.require(ctx.config,parents)?;
        Ok(result)
    }

    pub(in crate::engine) fn require(&self, cfg: &ValidatedConfig, parents: &OrdinaryDirectories) -> Result<()> {
        require(fresh_layout::registration(cfg,&self.disk) == self.registration)?;
        fresh_layout::verify_root(cfg,&self.disk,&self.root.root)?;
        parents.require_owner(&self.root, &self.registration)?;
        parents.require(cfg)?;
        for f in self.files.values() {
            parents.require_file_entry(f.parent,&f.name,&f.rows.holder,f.entity)?;
            f.bodies.recheck(&f.rows)?;
        }
        Ok(())
    }
    pub(in crate::engine) fn append_to(&self, out: &mut OrdinalBuilder) -> Result<()> {
        for (&file,f) in &self.files {
            out.push(&InventoryRow { object: Object::FileContainer { file_id: file, material: &f.material },
                ownership: Ownership::Original { registration: &self.registration },
                location: Location::Entry { root_id: f.rows.replica().root_id, parent_directory_id: f.parent, name: f.name.as_str() },
                lifecycle: Lifecycle::Current {}, entity: f.entity, operation_id: f.creation })?;
        }
        Ok(())
    }
    pub(in crate::engine) fn verify_retained(&self, cfg: &ValidatedConfig, parents: &OrdinaryDirectories) -> Result<()> {
        self.require(cfg,parents)?;
        for f in self.files.values() {
            f.bodies.verify_retained(cfg, &self.disk, &self.root.root, &f.rows, &f.material)?;
        }
        self.require(cfg,parents)
    }
}
