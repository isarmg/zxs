//! One immutable admission cut for explicit FILE occurrences in sibling sparse
//! mounts of a *different* directory repository. No discover-all, no persisted
//! shadow registry, no source edit. Caller must authorize both the outer write
//! and the actual selected inner resources; this guard is not that grant.
pub(crate) mod projection;
use super::{HeldBranchFile, InnerOccurrence, Pick};
use crate::persist::snapshot_worktree::HostEvidence;
use crate::snapshot_worktree::{RegistrationError, Result, instance::MountParent, registry::ReceiptIndex};
use crate::working_tree::snapshot::{HeldSnapshotNode, SnapshotNodeBinding};
use crate::{Engine, ReadFile, State};
use name_policy::PortablePath;
use std::{collections::BTreeSet, sync::Arc};
use storage_contract::snapshot_worktree::{NamespaceDirectoryId, SnapshotHandle};
use storage_contract::snapshot_worktree::discovery::{SnapshotBranchSelection, SnapshotHeadDiscovery, SnapshotHeadRequest};

pub(crate) struct HeldMountedFiles {
    cut: Arc<State>,
    outer: SnapshotHandle,
    host: HostEvidence,
    node: HeldSnapshotNode,
    rows: Vec<(PortablePath,HeldBranchFile)>,
    _permit: crate::engine::PayloadProofReservation,
}
impl HeldMountedFiles {
    pub(crate) fn capture(engine:&Engine, outer:SnapshotHandle, picks:&[Pick<'_>]) -> Result<Self> {
        if picks.is_empty() || picks.len() > engine.config().max_entries
            || picks.len() > crate::working_tree::snapshot::MAX_ENTRIES { return Err(RegistrationError::CommitScope); }
        let permit = crate::engine::payload_proof_budget().reserve(8).ok_or_else(||
            RegistrationError::Publication(crate::engine::fresh_layout::FreshLayoutError::ContainerBudget.into()))?;
        let index = engine.snapshot_receipts()?;
        let (host,node) = outer_head(engine,&index,outer)?;
        let mut rows = Vec::with_capacity(picks.len());
        let mut paths = BTreeSet::new();
        for pick in picks {
            let inner = HeldBranchFile::capture_at(engine,&index,pick)?;
            let path = mounted_path(outer,&host,inner.facts())?;
            if !paths.insert(path.key().to_owned()) { return Err(RegistrationError::Mount); }
            rows.push((path,inner));
        }
        // Request order never selects a second owner of the same path.
        rows.sort_by(|a,b| a.0.key().cmp(b.0.key()));
        Ok(Self { cut:Arc::clone(&engine.state),outer,host,node,rows,_permit:permit })
    }

    pub(crate) fn outer(&self) -> (SnapshotHandle,&HostEvidence,SnapshotNodeBinding) {
        (self.outer,&self.host,self.node.binding())
    }
    pub(crate) fn occurrences(&self) -> impl ExactSizeIterator<Item=(&PortablePath,&InnerOccurrence)> {
        self.rows.iter().map(|(path,source)| (path,source.facts()))
    }
    pub(crate) fn bodies(&mut self) -> impl ExactSizeIterator<Item=(&PortablePath,&mut ReadFile)> {
        self.rows.iter_mut().map(|(path,source)| (&*path,source.reader()))
    }

    /// Revalidate the complete single-writer cut. A fresh request may capture
    /// again; an already decided original must use its own recorded outcome.
    pub(crate) fn require_current(&self, engine:&Engine) -> Result<()> {
        if !Arc::ptr_eq(&self.cut,&engine.state) { return Err(RegistrationError::PublicationChanged); }
        let index = engine.snapshot_receipts()?;
        let (host,node) = outer_head(engine,&index,self.outer)?;
        if host != self.host || node.binding() != self.node.binding() { return Err(RegistrationError::PublicationChanged); }
        for (path,old) in &self.rows {
            let f = old.facts();
            let pick = Pick { worktree:f.worktree,visible:f.visible,revision:f.revision,path:&f.displayed };
            let fresh = HeldBranchFile::capture_at(engine,&index,&pick)?;
            if fresh.facts() != f || mounted_path(self.outer,&host,fresh.facts())? != *path {
                return Err(RegistrationError::PublicationChanged);
            }
            old.body.require_named().map_err(RegistrationError::Publication)?;
        }
        Ok(())
    }
}

fn outer_head(engine:&Engine,index:&ReceiptIndex<'_>,outer:SnapshotHandle) -> Result<(HostEvidence,HeldSnapshotNode)> {
    index.require_source(engine.state())?;
    let original = index.node(outer)?;
    let host = original.host();
    if !host.directories.is_empty() || host.namespace.directory != NamespaceDirectoryId::Repository(host.namespace.repository_id) {
        return Err(RegistrationError::RootOnly);
    }
    let root = engine.namespace_host_root(host.namespace.repository_id,host.namespace.branch_id).map_err(RegistrationError::Publication)?;
    match index.discover_root(SnapshotHeadRequest { namespace:host.namespace, host:original.initialization().host,
        branch:SnapshotBranchSelection::Exact { repository:outer.repository_id,branch:outer.branch_id }
    },root.binding())? {
        SnapshotHeadDiscovery::Head {snapshot,..} if snapshot == outer => {}
        SnapshotHeadDiscovery::Pending {operation} => return Err(RegistrationError::Pending(operation)),
        _ => return Err(RegistrationError::PublicationChanged),
    }
    Ok((host.clone(),index.reopen_node(engine,outer)?))
}

fn mounted_path(outer:SnapshotHandle,host:&HostEvidence,inner:&InnerOccurrence) -> Result<PortablePath> {
    let parent = MountParent::original(&inner.host)?;
    if inner.source.repository_id == outer.repository_id || inner.host.directories.len() != 1
        || parent.namespace != host.namespace || parent.directory != inner.mount.parent
        || host.common_data != inner.host.common_data || host.registration != inner.host.registration
        || host.root_entity != inner.host.root_entity { return Err(RegistrationError::Host); }
    let path = PortablePath::parse(inner.mount.name.as_str()).and_then(|mount| inner.displayed.rebase(0,&mount))
        .map_err(|e| RegistrationError::WorktreeName(crate::directory_worktree::names::NameError::MountedPath(e)))?;
    Ok(path)
}
