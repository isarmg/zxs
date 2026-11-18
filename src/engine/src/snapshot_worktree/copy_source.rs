//! Immutable bytes in an exact, already registered .zxs node. A caller cannot
//! provide a raw FD, manifest or Version hash. This material is not proof that
//! a particular Worktree occurrence was included, is still visible, or may
//! be removed: the enclosing operation has those separate native obligations.
use super::{RegistrationError as Failure, Result};
use crate::disk::DiskResources;
use crate::engine::{PayloadProofReservation, PlanningContext};
use crate::persist::snapshot_worktree::HostEvidence;
use crate::working_tree::snapshot::{
    HeldSnapshotNode, SnapshotEntry, SnapshotEntryContent, SnapshotFile, SnapshotFileSource,
    SnapshotNodeBinding,
};
use crate::{Engine, ReadFile, State, ValidatedConfig};
use name_policy::PortablePath;
use platform_io::fs_safety::linux::FileIdentity;
use std::{path::{Path, PathBuf}, sync::{Arc, Weak}};
use storage_contract::snapshot_worktree::SnapshotHandle;

pub(crate) struct PublishedSnapshotFile {
    source: Weak<State>,
    config: ValidatedConfig,
    resources: Weak<DiskResources>,
    snapshot: Option<String>,
    slot: u64,
    handle: SnapshotHandle,
    host: HostEvidence,
    node: HeldSnapshotNode,
    occurrence: PortablePath,
    material: SnapshotFile,
    origin: SnapshotFileSource,
    address: PathBuf,
    entity: FileIdentity,
    original: ReadFile,
    _permit: PayloadProofReservation,
}

impl PublishedSnapshotFile {
    /// A previously prepared but unregistered local node cannot cross this
    /// boundary. Equal hashes under a second path are not the selected row.
    pub(crate) fn capture(engine: &Engine, handle: SnapshotHandle, path: &PortablePath) -> Result<Self> {
        let context = engine.planning_context();
        if context.current_namespaces.is_some() || context.config.generation().is_some() {
            return Err(Failure::InitialOnly);
        }
        let index = engine.snapshot_receipts()?;
        let registered = index.node(handle)?;
        let selected = occurrence(registered.manifest().entries(), path)?;
        let SnapshotEntryContent::File { material, source: origin } = selected.content else {
            return Err(Failure::Source);
        };
        if material.size > context.config.max_file_bytes { return Err(Failure::CommitScope); }
        let host = registered.host().clone();
        let count = host.directories.len().checked_add(8).ok_or(Failure::CommitScope)?;
        let permit = crate::engine::payload_proof_budget().reserve(count).ok_or_else(||
            Failure::Publication(crate::engine::fresh_layout::FreshLayoutError::ContainerBudget.into()))?;
        let node = index.reopen_node(engine, handle)?;
        if node.binding() != registered.binding() || occurrence(node.manifest().entries(), path)? != selected {
            return Err(Failure::Source);
        }
        let (address, original) = node.named_copy_payload(material.version_id).map_err(Failure::NodeReadback)?;
        let entity = FileIdentity::from_metadata(&original.metadata().map_err(Failure::CommitRead)?);
        Ok(Self {
            source: Arc::downgrade(context.state), config: context.config.clone(),
            resources: Arc::downgrade(context.disk_resources), snapshot: context.snapshot.clone(),
            slot: context.next_slot, handle, host, node, occurrence: path.clone(), material,
            origin, address, entity, original, _permit: permit,
        })
    }

    pub(crate) fn handle(&self) -> SnapshotHandle { self.handle }
    pub(crate) fn binding(&self) -> SnapshotNodeBinding { self.node.binding() }
    pub(crate) fn occurrence(&self) -> &PortablePath { &self.occurrence }
    pub(crate) fn material(&self) -> SnapshotFile { self.material }
    pub(crate) fn origin(&self) -> SnapshotFileSource { self.origin }
    pub(crate) fn host(&self) -> &HostEvidence { &self.host }
    pub(crate) fn address(&self) -> &Path { &self.address }

    pub(crate) fn require_context(&self, context: &PlanningContext<'_>) -> Result<()> {
        if !Weak::ptr_eq(&self.source, &Arc::downgrade(context.state))
            || !Weak::ptr_eq(&self.resources, &Arc::downgrade(context.disk_resources))
            || !self.config.same_admission(context.config) || self.slot != context.next_slot
            || self.snapshot != *context.snapshot || context.current_namespaces.is_some()
        {
            return Err(Failure::PublicationChanged);
        }
        Ok(())
    }

    /// Source-side pre-decision check through the current registered host,
    /// not just through an old directory descriptor. No later current FILE
    /// node can be substituted for the selected .zxs occurrence.
    pub(crate) fn require_current(&self, engine: &Engine) -> Result<()> {
        self.require_context(&engine.planning_context())?;
        let index = engine.snapshot_receipts()?;
        let registration = index.node(self.handle)?;
        if registration.host() != &self.host || registration.binding() != self.node.binding() {
            return Err(Failure::Source);
        }
        let actual = index.reopen_node(engine, self.handle)?;
        let entry = occurrence(actual.manifest().entries(), &self.occurrence)?;
        if entry.content != (SnapshotEntryContent::File { material: self.material, source: self.origin }) {
            return Err(Failure::Source);
        }
        self.open_at(&actual, &self.address)?;
        Ok(())
    }

    pub(crate) fn open(&self, address: &Path) -> crate::Result<ReadFile> {
        self.open_at(&self.node, address).map_err(crate::business::file_draft::worktree::failure)
    }

    fn open_at(&self, node: &HeldSnapshotNode, address: &Path) -> Result<ReadFile> {
        if address != self.address || node.binding() != self.node.binding() { return Err(Failure::Source); }
        let (actual, file) = node.named_copy_payload(self.material.version_id).map_err(Failure::NodeReadback)?;
        if actual != self.address || FileIdentity::from_metadata(&file.metadata().map_err(Failure::CommitRead)?) != self.entity
            || FileIdentity::from_metadata(&self.original.metadata().map_err(Failure::CommitRead)?) != self.entity
        {
            return Err(Failure::Source);
        }
        Ok(file)
    }
}

fn occurrence<'a>(entries: &'a [SnapshotEntry], path: &PortablePath) -> Result<&'a SnapshotEntry> {
    let index = entries.binary_search_by(|entry| entry.path.key().cmp(path.key())).map_err(|_| Failure::Source)?;
    let entry = &entries[index];
    if entry.path != *path { return Err(Failure::Source); }
    Ok(entry)
}
