//! All directory subgraphs under one actual original disk registration. An
//! empty result is justified by the complete native index, not an empty
//! directory, an unavailable device or an omission from the selected write.
use super::*;
use crate::engine::scope_closure::OrdinalBuilder;
use crate::persist::snapshot_worktree::Record;
use crate::snapshot_worktree::LocalDirectory;
use std::collections::BTreeMap;
use storage_contract::{DiskId, Principal, working_tree::DiskRegistrationIdentity};

enum Entry { Pending(PendingRepository), Bound(PublishedRepository) }

pub(in crate::engine) struct DirectoryCensus {
    registration: DiskRegistrationIdentity, disk: DiskConfig, root: Arc<DiskHandle>,
    repositories: BTreeMap<SnapshotRepositoryId, Entry>,
}
impl DirectoryCensus {
    pub(in crate::engine) fn original(ctx: &PlanningContext<'_>, disk: &DiskConfig) -> Result<Self> {
        let (registration, root) = Self::holder(ctx, disk)?;
        let index = ctx.snapshot_receipts().map_err(crate::business::file_draft::worktree::failure)?;
        let sources = index.original_directories(&registration).map_err(crate::business::file_draft::worktree::failure)?;
        Self::from_sources(ctx, disk, registration, root, sources)
    }
    pub(in crate::engine) fn planned(ctx: &PlanningContext<'_>, disk: &DiskConfig, next: &crate::State,
        receipt: &crate::OperationResult, principal: &Principal, record: &Record) -> Result<Self> {
        check(ctx.next_slot == receipt.sequence)?;
        let (registration, root) = Self::holder(ctx, disk)?;
        let index = ctx.snapshot_receipts().map_err(crate::business::file_draft::worktree::failure)?;
        index.with_planned_directories(next, receipt, principal, record, &registration, |sources|
            Self::from_sources(ctx, disk, registration.clone(), root, sources))
            .map_err(crate::business::file_draft::worktree::failure)?
    }
    pub(super) fn holder(ctx: &PlanningContext<'_>, disk: &DiskConfig) -> Result<(DiskRegistrationIdentity, Arc<DiskHandle>)> {
        let id = disk.disk_id.parse::<DiskId>()?;
        let binding = ctx.config.storage_roots.get(&id).ok_or_else(reject)?;
        let reg = fresh_layout::registration(ctx.config, disk);
        check(ctx.config.disks.iter().any(|d| d == disk) && binding.registration == reg)?;
        let root = Arc::clone(ctx.disk_resources.get(disk)?);
        fresh_layout::verify_root(ctx.config, disk, &root.root)?;
        Ok((reg, root))
    }
    pub(super) fn from_sources(ctx: &PlanningContext<'_>, disk: &DiskConfig, registration: DiskRegistrationIdentity,
        root: Arc<DiskHandle>, sources: BTreeMap<SnapshotRepositoryId, LocalDirectory<'_>>) -> Result<Self> {
        let mut repositories = BTreeMap::new();
        let mut bytes = 0usize;
        for (id, source) in sources {
            let entry = match source {
                LocalDirectory::Reserved(facts) => {
                    check(facts.allocated.repository == id && facts.host.registration == registration)?;
                    Entry::Pending(PendingRepository::from_facts(ctx, facts)?)
                }
                LocalDirectory::Bound(facts) => {
                    check(facts.initialization.allocated.repository == id && facts.initialization.host.registration == registration)?;
                    Entry::Bound(PublishedRepository::from_facts(ctx, facts)?)
                }
            };
            let n = match &entry { Entry::Pending(p) => p.bytes()?, Entry::Bound(p) => p.bytes()? };
            bytes = bytes.checked_add(n).filter(|n| *n <= 64 * 1024 * 1024).ok_or(Failure::Budget)?;
            repositories.insert(id, entry);
        }
        let value = Self { registration, disk: disk.clone(), root, repositories };
        value.require(ctx.config)?;
        Ok(value)
    }
    pub(in crate::engine) fn registration(&self) -> &DiskRegistrationIdentity { &self.registration }
    pub(in crate::engine) fn require(&self, config: &ValidatedConfig) -> Result<()> {
        check(fresh_layout::registration(config, &self.disk) == self.registration)?;
        fresh_layout::verify_root(config, &self.disk, &self.root.root)?;
        for r in self.repositories.values() {
            match r { Entry::Pending(p) => p.require(config)?, Entry::Bound(p) => p.require(config)? }
        }
        Ok(())
    }
    pub(in crate::engine) fn append_to(&self, config: &ValidatedConfig, rows: &mut OrdinalBuilder) -> Result<()> {
        self.require(config)?;
        for r in self.repositories.values() {
            match r { Entry::Pending(p) => p.append_to(rows)?, Entry::Bound(p) => p.append_to(config, rows)? }
        }
        Ok(())
    }
    pub(in crate::engine) fn stage_original(&self, config: &ValidatedConfig) -> Result<()> {
        self.require(config)?;
        for r in self.repositories.values() {
            match r { Entry::Pending(p) => p.stage_original(config)?, Entry::Bound(p) => p.stage_original(config)? }
        }
        self.require(config)
    }
    pub(in crate::engine) fn verify_original(&self, config: &ValidatedConfig) -> Result<()> {
        self.require(config)?;
        for r in self.repositories.values() {
            match r { Entry::Pending(p) => p.verify_original(config)?, Entry::Bound(p) => p.verify_original(config)? }
        }
        self.require(config)
    }
}

/// All supported ordinary directories AND directory repository/instance
/// entries, with one authentication of the selected prefix. A mixed-disk
/// publisher must still supply FILE, native disk images, changes, etc.
pub(in crate::engine) struct NamespaceCensus {
    pub(in crate::engine) ordinary: super::OrdinaryDirectories,
    pub(in crate::engine) directories: DirectoryCensus,
}
impl NamespaceCensus {
    pub(in crate::engine) fn original(ctx: &PlanningContext<'_>, disk: &DiskConfig) -> Result<Self> {
        let (registration, root) = DirectoryCensus::holder(ctx,disk)?;
        let binding = ctx.config.storage_roots.get(&disk.disk_id.parse::<DiskId>()?).ok_or_else(reject)?;
        let index = ctx.snapshot_receipts().map_err(crate::business::file_draft::worktree::failure)?;
        let ordinary = index.ordinary_originals(binding).map_err(crate::business::file_draft::worktree::failure)?;
        let directories = index.original_directories(&registration).map_err(crate::business::file_draft::worktree::failure)?;
        let view = Self {
            ordinary: super::OrdinaryDirectories::from_originals(ctx, disk, ordinary)?,
            directories: DirectoryCensus::from_sources(ctx, disk, registration, root, directories)? };
        view.require(ctx.config)?;
        Ok(view)
    }
    pub(in crate::engine) fn planned(ctx: &PlanningContext<'_>, disk: &DiskConfig, next: &crate::State,
        receipt: &crate::OperationResult, principal: &Principal, record: &Record,
        pair: &crate::engine::disk_catalog::FrozenDiskPair) -> Result<Self> {
        let (registration, root) = DirectoryCensus::holder(ctx,disk)?;
        check(ctx.next_slot == receipt.sequence && pair.registration == registration)?;
        pair.require_planned_publication(ctx.config, ctx.snapshot.as_deref(), receipt)?;
        let binding = ctx.config.storage_roots.get(&disk.disk_id.parse::<DiskId>()?).ok_or_else(reject)?;
        let index = ctx.snapshot_receipts().map_err(crate::business::file_draft::worktree::failure)?;
        index.with_planned_namespaces(next, receipt, principal, record, binding, |ordinary, directories| {
            let view = Self {
                ordinary: super::OrdinaryDirectories::from_originals(ctx,disk,ordinary)?,
                directories: DirectoryCensus::from_sources(ctx,disk,registration,root,directories)? };
            view.require(ctx.config)?;
            Ok(view)
        }).map_err(crate::business::file_draft::worktree::failure)?
    }
    pub(in crate::engine) fn require(&self, config: &ValidatedConfig) -> Result<()> {
        self.ordinary.require(config)?;
        self.directories.require(config)
    }
    pub(in crate::engine) fn append_to(&self, config: &ValidatedConfig, rows: &mut OrdinalBuilder) -> Result<()> {
        self.ordinary.append_to(config, rows)?;
        self.directories.append_to(config, rows)
    }
}

impl NamespaceCensus {
    /// Complete namespace section on the original registered local holder;
    /// deliberately not a whole disk-health result or a recovery decision.
    pub(in crate::engine) fn verify_retained(&self, ctx: &PlanningContext<'_>,
        selected: &crate::engine::scope_closure::HistoryReference) -> Result<usize> {
        let disk = &self.directories.disk;
        check(Arc::ptr_eq(&self.directories.root, ctx.disk_resources.get(disk)?))?;
        self.require(ctx.config)?;
        let mut actual = OrdinalBuilder::new(65_536);
        self.append_to(ctx.config, &mut actual)?;
        let n = selected.require_section(disk, &self.directories.root.root, self.directories.registration(),
            ctx.state.sequence, crate::engine::scope_closure::inventory::Section::Namespace, actual)?;
        self.directories.verify_original(ctx.config)?;
        self.require(ctx.config)?;
        Ok(n)
    }
}
