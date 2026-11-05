//! Carry an already admitted namespace through a registry-bearing Add or Abort.
//! This prepares data only; the publication verifier still owns admission.
use super::*;
use crate::engine::disk_registry::current::{
    CurrentNamespaces, CurrentScope, PreparedCurrentNamespaces,
};
use std::sync::Arc;

impl CurrentScope {
    fn at_next_source(&self) -> Self {
        Self {
            head: self.head.clone(),
            entries: Arc::clone(&self.entries),
            source_head: self.head.clone(),
            source_entries: Arc::clone(&self.entries),
            writable: self.writable,
            // A published new head has already materialized the clean entries.
            // Carrying its earlier removed-path list would clear newly recreated
            // same-name contexts during the next generation's first write.
            hidden_paths: if self.head == self.source_head {
                self.hidden_paths.clone()
            } else {
                BTreeSet::new()
            },
        }
    }
}

impl CurrentNamespaces {
    pub(in crate::engine) fn prepare_continuation(
        &self,
        source: &crate::State,
        snapshot: &str,
        successor: &VerifiedDiskRegistry,
    ) -> Result<PreparedCurrentNamespaces> {
        digest(snapshot)?;
        if !self.matches_source(source) || source.sequence < self.activation_sequence {
            return Err(Error::new(
                crate::ErrorCode::PublicationInvalid,
                "registry continuation has no exact admitted source namespace",
            ));
        }
        // Abort can inherit exactly the parent's registry. Add uses the complete
        // continuation already checked against source and target configuration
        // by authority; here its parent/source binding must still be the view
        // actually being carried, not another valid registry graph.
        if successor.commitment() != &self.registry
            && (successor.state.predecessor.as_ref() != Some(&self.registry)
                || Some(successor.commitment.view_generation())
                    != self.registry.view_generation().checked_add(1)
                || successor.state.pending_exit.is_some()
                || successor.state.source.snapshot_hash.as_deref() != Some(snapshot)
                || successor.state.source.sequence != source.sequence
                || successor.state.source.topology_generation != source.topology_generation)
        {
            return Err(Error::new(
                crate::ErrorCode::PublicationInvalid,
                "registry continuation differs from its admitted parent and source",
            ));
        }
        let mut view = self.clone();
        view.registry = successor.commitment.clone();
        view.registry_state = Arc::new(successor.clone());
        view.mask_hash = None;
        view.source_snapshot = snapshot.into();
        view.source_sequence = source.sequence;
        view.source_business_generation = source.business_generation;
        view.activation_sequence = source.sequence;
        view.branches = Arc::new(
            self.branches
                .iter()
                .map(|(id, scope)| (id.clone(), Arc::new(scope.at_next_source())))
                .collect(),
        );
        view.directories = Arc::new(
            self.directories
                .iter()
                .map(|(id, scope)| (id.clone(), Arc::new(scope.at_next_source())))
                .collect(),
        );
        Ok(PreparedCurrentNamespaces { view })
    }
}

impl CurrentNamespaces {
    /// Carry only the already admitted current entries. Detached masks, hidden
    /// identities and unavailable versions are never reconstructed from history.
    pub(in crate::engine) fn prepare_readmission(
        &self,
        source: &crate::State,
        snapshot: &str,
        successor: &VerifiedDiskRegistry,
        identity: &crate::DiskRegistrationIdentity,
    ) -> Result<PreparedCurrentNamespaces> {
        if self.registry_state.registration(identity)? != Some(crate::RegistrationMode::Disabled)
            || successor.registration(identity)? != Some(crate::RegistrationMode::Enabled)
        {
            return Err(Error::new(
                crate::ErrorCode::PublicationInvalid,
                "namespace readmission is not Disabled to Enabled",
            ));
        }
        let mut prepared = self.prepare_continuation(source, snapshot, successor)?;
        for (id, scope) in Arc::make_mut(&mut prepared.view.branches) {
            let branch = source
                .branches
                .get(id)
                .expect("invariant: continuation retains source branches");
            let history = source
                .histories
                .get(&branch.history_tree_id)
                .ok_or_else(|| {
                    Error::new(
                        crate::ErrorCode::PublicationInvalid,
                        "readmitted branch has no history",
                    )
                })?;
            let repository = source
                .repositories
                .get(&history.repository_id)
                .ok_or_else(|| {
                    Error::new(
                        crate::ErrorCode::PublicationInvalid,
                        "readmitted branch has no repository",
                    )
                })?;
            if repository.primary_slot_id == identity.slot_id {
                Arc::make_mut(scope).writable = true;
            }
        }
        for (id, scope) in Arc::make_mut(&mut prepared.view.directories) {
            let branch = source
                .directory_branches
                .get(id)
                .expect("invariant: continuation retains source directories");
            let repository = source
                .repositories
                .get(&branch.repository_id)
                .ok_or_else(|| {
                    Error::new(
                        crate::ErrorCode::PublicationInvalid,
                        "readmitted directory has no repository",
                    )
                })?;
            if repository.primary_slot_id == identity.slot_id {
                Arc::make_mut(scope).writable = true;
            }
        }
        Ok(prepared)
    }
}
