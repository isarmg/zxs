//! First topology only. A real future repository host move/detach will have
//! its own exact material handoff; ordinary rename/remove cannot impersonate it.
use super::{Publication, ReceiptIndex};
use crate::persist::snapshot_worktree::{HostEvidence, NamespaceDirectoryBinding, NodeReservation};
use crate::snapshot_worktree::{RegistrationError, Result};
use crate::State;
use storage_contract::file_container::NamespaceDirectoryParent;
use storage_contract::snapshot_worktree::NamespaceDirectoryId;

fn require_owner(state: &State, host: &HostEvidence, child: &NamespaceDirectoryBinding) -> Result<()> {
    let native = state.namespace_directories.get(&child.entry.to_string()).ok_or(RegistrationError::DirectChildHost)?;
    if native.repository != host.namespace.repository_id || native.branch != host.namespace.branch_id
        || native.parent != NamespaceDirectoryParent::Root || native.owner_disk.to_string() != host.registration.disk_id
        || native.entry != child.entry || native.name != child.directory.name
        || !crate::business::namespace::NamespaceTree::contains_registered_direct_child(state, native)
    { return Err(RegistrationError::DirectChildHost); }
    let owner = crate::engine::fresh_layout::directories::owner_binding(state,native).map_err(RegistrationError::Publication)?;
    if owner != child.directory || owner.parent_directory_id != host.common_data.directory_id
        || owner.lifecycle != storage_contract::working_tree::BindingLifecycle::Current
        || native.replicas.get(&native.owner_disk).is_none_or(|r|
            r.parent_entity.device() != host.common_data.entity.device() || r.parent_entity.inode() != host.common_data.entity.inode())
    { return Err(RegistrationError::DirectChildHost); }
    Ok(())
}

impl ReceiptIndex<'_> {
    pub(super) fn require_direct_child_reserve(&self, plan: &NodeReservation) -> Result<()> {
        let Some(path) = &plan.request.source.root else { return Ok(()); };
        let [child] = plan.host.directories.as_slice() else { return Err(RegistrationError::RootOnly); };
        if plan.host.namespace.directory != NamespaceDirectoryId::Entry(child.entry)
            || path.as_str() != child.directory.name.as_str() || child.directory.operation_id == plan.request.operation_id
        { return Err(RegistrationError::Host); }
        // The outer registration must already be complete at this historical
        // position. A first child can neither choose nor construct its own parent.
        let outer = self.nodes.values().find(|node| {
            let parent = &node.plan;
            parent.request.host.root_id == plan.request.host.root_id
                && parent.request.host.directory_id == plan.host.common_data.directory_id
        }).ok_or(RegistrationError::Host)?;
        let parent = &outer.plan;
        if !matches!(outer.publication, Publication::Complete(_))
            || parent.host.namespace.directory != NamespaceDirectoryId::Repository(plan.host.namespace.repository_id)
            || parent.host.namespace.repository_id != plan.host.namespace.repository_id
            || parent.host.namespace.branch_id != plan.host.namespace.branch_id || !parent.host.directories.is_empty()
            || parent.host.common_data != plan.host.common_data || parent.host.registration != plan.host.registration
            || parent.host.root_entity != plan.host.root_entity
            || parent.request.host.expected_root_generation != plan.request.host.expected_root_generation
        { return Err(RegistrationError::Host); }
        require_owner(self.source, &plan.host, child)
    }

    pub(super) fn require_direct_child_hosts(&self, next: &State) -> Result<()> {
        for original in self.nodes.values() {
            if let [child] = original.plan.host.directories.as_slice() {
                require_owner(next, &original.plan.host, child)?;
            }
        }
        Ok(())
    }
}
