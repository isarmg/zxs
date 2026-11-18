//! A same-process reserve continuation consumes the original held host. Cold
//! Pending is deliberately not a constructor: a matching directory is not it.
use super::PreparedNamespaceHost;
use crate::container_storage::namespace_host::{NamespaceHost, NamespaceHostError};
use crate::disk::DirectoryCreateOutcome;
use crate::persist::layout;
use crate::snapshot_worktree::{PublishedNodeReservation, RegistrationError};
use crate::working_tree::records::local_entity;
use crate::working_tree::snapshot::{
    AdmittedSnapshotManifest, HeldSnapshotNode, SnapshotNodeBinding,
};
use crate::Engine;
use std::sync::Arc;

type Result<T> = std::result::Result<T, RegistrationError>;

pub(crate) struct PreparedNamespaceNode {
    host: PreparedNamespaceHost,
    reservation: Arc<PublishedNodeReservation>,
    node: HeldSnapshotNode,
}

impl PreparedNamespaceHost {
    pub(crate) fn require_snapshot_source(
        &self,
        engine: &Engine,
    ) -> std::result::Result<(), NamespaceHostError> {
        self.require_engine(engine)?;
        let source = NamespaceHost::for_request(engine, &self.source_request)?;
        let selected = crate::snapshot_worktree::direct_child::Window::new(
            &self.snapshot, &self.evidence, self.source_request.root.as_ref(),
        )?;
        let (count, rows) = selected.rows(&self.snapshot);
        if count > crate::working_tree::snapshot::MAX_ENTRIES {
            return Err(NamespaceHostError::RootInventory);
        }
        let common = source.held_root(self.root,self.generation)?;
        let host = self.directories.last().unwrap_or(common);
        source.require_inventory(host, false, rows)
    }

    pub(crate) fn prepare_node(
        mut self,
        engine: &mut Engine,
        reservation: Arc<PublishedNodeReservation>,
        manifest: AdmittedSnapshotManifest,
    ) -> Result<PreparedNamespaceNode> {
        reservation.require_host(
            engine, &self.state, self.operation, &self.evidence, &self.observation(),
        )?;
        if manifest.reference() != reservation.material() {
            return Err(RegistrationError::Material);
        }
        // This is the only source transition. Its input is the actual successful
        // publication product for this exact before/after pair and prepared host.
        self.state = Arc::clone(reservation.after());
        self.snapshot = reservation.after_namespace().clone();
        self.require_snapshot_source(engine).map_err(RegistrationError::NamespaceHost)?;
        reservation.require_ready(engine, &manifest)?;
        let source = NamespaceHost::for_request(engine, &self.source_request)
            .map_err(RegistrationError::NamespaceHost)?;
        let common = source.held_root(self.root, self.generation)
            .map_err(RegistrationError::NamespaceHost)?;
        // The nested repository is private to this actual direct child, not a
        // second repository under the enclosing root's .zxs.
        let host = self.directories.last().unwrap_or(common);
        let created = host.create_private_entry(&layout::SNAPSHOT_REPOSITORY);
        let (repository, identity) = match created {
            DirectoryCreateOutcome::Rejected(source)
            | DirectoryCreateOutcome::Uncertain { source, .. } => {
                return Err(RegistrationError::RepositoryPreparation(source));
            }
            DirectoryCreateOutcome::Created { directory, identity, durability } => {
                durability.map_err(RegistrationError::RepositoryPreparation)?;
                host.sync_private_entry_matching(&layout::SNAPSHOT_REPOSITORY, identity)
                    .map_err(RegistrationError::RepositoryPreparation)?;
                (directory, identity)
            }
        };
        let node = HeldSnapshotNode::prepare(
            &repository, manifest, |file| reservation.open_payload(engine, file),
        ).map_err(RegistrationError::NodePreparation)?;
        if local_entity(repository.held_identity()
            .map_err(RegistrationError::RepositoryPreparation)?) != local_entity(identity)
        {
            return Err(RegistrationError::PublicationChanged);
        }
        let prepared = PreparedNamespaceNode { host: self, reservation, node };
        prepared.require_engine(engine)?;
        Ok(prepared)
    }
}

impl PreparedNamespaceNode {
    pub(crate) fn reservation(&self) -> &Arc<PublishedNodeReservation> {
        &self.reservation
    }

    pub(crate) fn binding(&self) -> SnapshotNodeBinding {
        self.node.binding()
    }

    pub(crate) fn require_engine(&self, engine: &Engine) -> Result<()> {
        if !Arc::ptr_eq(&engine.state, self.reservation.after()) {
            return Err(RegistrationError::PublicationChanged);
        }
        self.host.require_engine(engine).map_err(RegistrationError::NamespaceHost)?;
        self.reservation.require_original_names()?;
        let source = NamespaceHost::for_request(engine, &self.host.source_request)
            .map_err(RegistrationError::NamespaceHost)?;
        let common = source.held_root(self.host.root, self.host.generation)
            .map_err(RegistrationError::NamespaceHost)?;
        let host = self.host.directories.last().unwrap_or(common);
        HeldSnapshotNode::reopen_at_host(host, self.node.binding())
            .map_err(RegistrationError::NodeReadback)?;
        Ok(())
    }
}
