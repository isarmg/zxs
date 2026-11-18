//! All currently supported ordinary directory occurrences on one original
//! holder. An index discharges logical coverage; held parent/name/entity
//! checks independently discharge availability. This is not a directory scan.
use super::{record::*, *};
use crate::engine::scope_closure::OrdinalBuilder;
use crate::persist::{layout::namespace as layout, snapshot_worktree::NamespaceDirectoryBinding};
use crate::snapshot_worktree::OrdinaryOriginal;
use name_policy::PortablePath;
use platform_io::fs_safety::linux::FileIdentity;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use storage_contract::{BranchId, DiskId, EntryId, RepositoryId, snapshot_worktree::{NamespaceDirectoryId, NamespaceHostAddress},
    working_tree::{BindingLifecycle, DirectoryId, DiskRegistrationIdentity, RootId}};
struct Held { namespace: Namespace, original: NamespaceDirectoryBinding, at: DiskRoot, pending: Option<OperationId> }
pub(in crate::engine) struct OrdinaryDirectories {
    disk: DiskConfig, root: Arc<DiskHandle>, registration: DiskRegistrationIdentity,
    root_id: RootId, anchor: DirectoryId, anchor_entity: FileIdentity, rows: BTreeMap<DirectoryId, Held>,
    _permit: crate::engine::PayloadProofReservation,
}
type Namespace = (RepositoryId, BranchId);
impl OrdinaryDirectories {
    pub(in crate::engine) fn original(ctx: &PlanningContext<'_>, disk: &DiskConfig) -> Result<Self> {
        let index = ctx.snapshot_receipts().map_err(crate::business::file_draft::worktree::failure)?;
        let binding = ctx.config.storage_roots.get(&disk.disk_id.parse::<DiskId>()?).ok_or_else(reject)?;
        let rows = index.ordinary_originals(binding).map_err(crate::business::file_draft::worktree::failure)?;
        Self::from_originals(ctx, disk, rows)
    }
    pub(super) fn from_originals(ctx: &PlanningContext<'_>, disk: &DiskConfig, source: BTreeMap<DirectoryId, OrdinaryOriginal<'_>>)
        -> Result<Self> {
        let b = ctx.config.storage_roots.get(&disk.disk_id.parse::<DiskId>()?).ok_or_else(reject)?;
        let registration = fresh_layout::registration(ctx.config, disk);
        check(ctx.config.disks.iter().any(|d| d == disk) && b.registration == registration)?;
        let root = Arc::clone(ctx.disk_resources.get(disk)?);
        fresh_layout::verify_root(ctx.config, disk, &root.root)?;
        let anchor = b.roots.common_data.directory_id;
        let anchor_entity = crate::working_tree::records::file_identity(b.roots.common_data.entity);
        let permit = crate::engine::payload_proof_budget().reserve(source.len()).ok_or(Failure::Budget)?;
        let mut expected = BTreeMap::<Namespace, BTreeSet<EntryId>>::new();
        let mut children = BTreeMap::<DirectoryId, Vec<DirectoryId>>::new();
        for (&id, x) in &source {
            let d = &x.binding.directory;
            check(d.directory_id == id && d.root_id == b.root_id && id != anchor && id != d.parent_directory_id
                && d.lifecycle == BindingLifecycle::Current)?;
            check(expected.entry((x.repository, x.branch)).or_default().insert(x.binding.entry))?;
            children.entry(d.parent_directory_id).or_default().push(id);
        }
        // Resolve original logical paths exactly once per admitted main
        // namespace. No repeated O(nodes*all entries) scans and no current
        // path can invent an obligation.
        let mut paths = BTreeMap::<(Namespace, EntryId), PortablePath>::new();
        for (ns, mut needed) in expected {
            let bound = ctx.namespace_host_root(ns.0, ns.1)?;
            check(bound.binding() == b && bound.disk() == disk)?;
            let addr = NamespaceHostAddress { repository_id: ns.0, branch_id: ns.1,
                directory: NamespaceDirectoryId::Repository(ns.0) };
            for e in ctx.scope_host_tree(addr)?.visible_entries().filter(|e| e.kind == crate::NamespaceEntryKind::Directory) {
                let id: EntryId = e.entry_id.parse()?;
                if !needed.remove(&id) { continue; }
                check(crate::container_storage::namespace_host::ordinary_path(&e.path))?;
                paths.insert((ns,id), e.path.clone());
            }
            check(needed.is_empty())?;
        }
        let common = root.content_root()?;
        check(common.directory_identity(common.path())? == anchor_entity)?;
        let mut rows = BTreeMap::<DirectoryId, Held>::new();
        let mut queue = VecDeque::from([anchor]);
        while let Some(parent) = queue.pop_front() {
            for id in children.remove(&parent).unwrap_or_default() {
                let x = source.get(&id).ok_or_else(reject)?;
                let d = &x.binding.directory;
                let ns = (x.repository, x.branch);
                let path = paths.get(&(ns, x.binding.entry)).ok_or_else(reject)?;
                let actual_parent = if parent == anchor {
                    check(path.parent().is_none())?; common
                } else {
                    let p = source.get(&parent).ok_or_else(reject)?;
                    check((p.repository,p.branch) == ns && path.parent().as_ref() == paths.get(&(ns,p.binding.entry)))?;
                    &rows.get(&parent).ok_or_else(reject)?.at
                };
                check(path.file_name() == d.name.as_str())?;
                let name = layout::directory(&d.name);
                let at = actual_parent.private_entry_matching(&name, crate::working_tree::records::file_identity(d.entity))
                    .map_err(directory_error)?;
                rows.insert(id, Held { namespace: ns, original: x.binding.clone(), at, pending: x.pending });
                queue.push_back(id);
            }
        }
        check(rows.len() == source.len() && children.is_empty())?;
        let held = Self { disk: disk.clone(), root, registration, root_id: b.root_id,
            anchor, anchor_entity, rows, _permit: permit };
        held.require(ctx.config)?;
        Ok(held)
    }
    pub(in crate::engine) fn require(&self, cfg: &ValidatedConfig) -> Result<()> {
        check(fresh_layout::registration(cfg, &self.disk) == self.registration)?;
        fresh_layout::verify_root(cfg, &self.disk, &self.root.root)?;
        let common = self.root.content_root()?;
        check(common.directory_identity(common.path())? == self.anchor_entity)?;
        for held in self.rows.values() {
            let dir = &held.original.directory;
            let p = if dir.parent_directory_id == self.anchor { common }
                else { &self.rows.get(&dir.parent_directory_id).ok_or_else(reject)?.at };
            let entity = crate::working_tree::records::file_identity(dir.entity);
            check(held.at.held_identity().map_err(directory_error)? == entity)?;
            p.private_entry_matching(&layout::directory(&dir.name), entity).map_err(directory_error)?;
        }
        Ok(())
    }
    pub(in crate::engine::scope_closure) fn require_owner(
        &self, root: &Arc<DiskHandle>, registration: &DiskRegistrationIdentity,
    ) -> Result<()> { check(Arc::ptr_eq(root,&self.root) && registration == &self.registration) }
    /// Join an original FILE to the exact already-admitted ordinary parent.
    /// A display path, backup prefix, logical deletion or Worktree cannot
    /// introduce an owner with a missing parent or an invented replica id.
    pub(in crate::engine::scope_closure) fn file_parent(
        &self, ctx: &PlanningContext<'_>, source: &storage_contract::file_container::FileContainerRecord,
    ) -> Result<DirectoryId> {
        use storage_contract::file_container::ContainerParent;
        let ns = &source.namespace;
        let id = self.disk.disk_id.parse::<DiskId>()?;
        let replica = source.replicas.get(&id).ok_or_else(reject)?;
        check(source.owner_disk == id && replica.root_id == self.root_id && replica.prefixes.is_empty())?;
        match &ns.parent {
            ContainerParent::Root => Ok(self.anchor),
            ContainerParent::Directory(entry) => {
                let d = ctx.state.namespace_directories.get(entry).ok_or_else(reject)?;
                let r = d.replicas.get(&id).ok_or_else(reject)?;
                let held = self.rows.get(&r.preparation).ok_or_else(reject)?;
                check(d.entry == *entry && d.owner_disk == id && held.pending.is_none()
                    && (ns.repository, ns.branch) == (d.repository, d.branch)
                    && held.namespace == (d.repository, d.branch) && held.original.entry == *entry
                    && r.root_id == self.root_id && r.source_chain.is_empty() && r.source_prefixes.is_empty()
                    && r.lifecycle == BindingLifecycle::Current && held.original.directory.entity ==
                        storage_contract::working_tree::LocalEntityIdentity::new(r.entity.device(), r.entity.inode()))?;
                Ok(r.preparation)
            }
            ContainerParent::Worktree { .. } => Err(reject()),
        }
    }

    pub(in crate::engine::scope_closure) fn require_file_entry(
        &self, parent: DirectoryId, name: &name_policy::LookupName,
        source: &crate::container_storage::ObservedContainerReplica, entity: storage_contract::working_tree::LocalEntityIdentity,
    ) -> Result<()> {
        check(Arc::ptr_eq(source.disk(), &self.root))?;
        let directory = if parent == self.anchor { self.root.content_root()? } else {
            let held = self.rows.get(&parent).ok_or_else(reject)?;
            check(held.pending.is_none())?;
            &held.at
        };
        let id = crate::working_tree::records::file_identity(entity);
        let named = directory.private_entry_matching(&layout::directory(name), id).map_err(directory_error)?;
        let original = source.container()?;
        check(named.path() == original.path() && original.held_identity().map_err(directory_error)? == id)
    }
    pub(in crate::engine) fn append_to(&self, cfg: &ValidatedConfig, out: &mut OrdinalBuilder) -> Result<()> {
        self.require(cfg)?;
        for (&id, held) in &self.rows {
            let d = &held.original.directory;
            out.push(&InventoryRow { object: Object::OrdinaryDirectory { directory_id: id },
                ownership: Ownership::Original { registration: &self.registration },
                location: Location::Entry { root_id: self.root_id, parent_directory_id: d.parent_directory_id, name: d.name.as_str() },
                lifecycle: held.pending.map_or(Lifecycle::Current {}, |reservation| Lifecycle::Pending { reservation }),
                entity: d.entity, operation_id: d.operation_id })?;
        }
        Ok(())
    }
}
fn directory_error(e: crate::disk::DirectoryIoError) -> crate::Error {
    use crate::disk::DirectoryIoError as D;
    match e { D::Gate(e) => e, D::Io(e) => e.into(), D::Filesystem(e) => e.into(), _ => reject() }
}
