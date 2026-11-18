//! One immutable native root beneath the pre-existing DISK inventory slot.
//! A prepared subgroup cannot certify a different repository or a whole disk.
mod record;
mod pending;
mod census;
pub(in crate::engine) use census::DirectoryCensus;
mod published;
mod instances;
pub(in crate::engine) use published::PublishedRepository;
pub(in crate::engine) use pending::PendingRepository;
use super::{
    OrdinalPages, Representation, ScopeClosureError as Failure,
    objects::{self, Domain, PreparedObject}, schema::{Material, Scope},
};
use crate::{DiskConfig, Result, ValidatedConfig, disk::{DiskHandle, DiskRoot}, engine::{PlanningContext, fresh_layout}};
use record::{Header, Host, Origin, RepositoryIdentity};
use std::sync::Arc;
use storage_contract::{
    ContentHash, OperationId, Sequence,
    snapshot_worktree::SnapshotHost,
    working_tree::{MaterialCommitment, SnapshotRepositoryId},
};

const MAX_ROWS: usize = 65_536;
fn reject() -> crate::Error { Failure::PublicationMismatch.into() }
fn check(ok: bool) -> Result<()> { if ok { Ok(()) } else { Err(reject()) } }
fn origin(receipt: &crate::OperationResult) -> Result<Origin> {
    check(receipt.sequence != 0)?;
    let hash = receipt.request_digest.parse::<ContentHash>().map_err(|_| reject())?;
    Ok(Origin { sequence: Sequence::new(receipt.sequence), operation_id: receipt.operation_id,
        request_digest: MaterialCommitment::from_digest(hash.digest()) })
}

fn host_error(e: crate::container_storage::namespace_host::NamespaceHostError) -> crate::Error {
    crate::business::file_draft::worktree::failure(crate::snapshot_worktree::RegistrationError::NamespaceHost(e))
}

struct BoundHost {
    native: Host,
    disk: DiskConfig,
    handle: Arc<DiskHandle>,
    chain: crate::container_storage::namespace_host::ScopedHostChain,
}
impl BoundHost {
    /// The initial registered ordinary root is actually held here. Pending
    /// never needs, opens or claims an unregistered .zxs repository.
    fn initial(ctx: &PlanningContext<'_>, requested: SnapshotHost,
        evidence: &crate::persist::snapshot_worktree::HostEvidence) -> Result<Self> {
        let ns = evidence.namespace;
        let root = ctx.namespace_host_root(ns.repository_id, ns.branch_id)?;
        let chain = crate::container_storage::namespace_host::ScopedHostChain::open(ctx, &root, requested, evidence)
            .map_err(host_error)?;
        let value = Self { native: Host { namespace: ns, registration: evidence.registration.clone(),
            root_id: requested.root_id, directory_id: requested.directory_id, host_entity: evidence.host().entity },
            disk: root.disk().clone(), handle: Arc::clone(ctx.disk_resources.get(root.disk())?), chain };
        value.check(ctx.config)?;
        Ok(value)
    }
    fn held(&self) -> &DiskRoot { self.chain.held() }
    fn append_steps(&self, selecting: Origin, rows: &mut super::ordinal::KeyedBuilder) -> Result<()> {
        for (i, original) in self.chain.steps().iter().enumerate() {
            // The native initialization selected this exact original binding
            // for THIS host. Its own (possibly earlier mkdir) creator remains
            // immutable inside original.directory.operation_id.
            rows.push(&record::Key::HostStep { ordinal: Sequence::new(i as u64) },
                &record::Row { source: selecting, value: record::HostStep { original } })?;
        }
        Ok(())
    }
    fn check(&self, config: &ValidatedConfig) -> Result<()> {
        fresh_layout::verify_root(config, &self.disk, &self.handle.root)?;
        let common = self.handle.content_root()?;
        let expected = crate::working_tree::records::file_identity(self.native.host_entity);
        self.chain.require(common).map_err(host_error)?;
        check(self.held().directory_identity(self.held().path())? == expected
            && fresh_layout::registration(config, &self.disk) == self.native.registration)
    }
}

struct FrozenDirectory {
    header: Header,
    descriptor: PreparedObject,
    rows: OrdinalPages,
    body: OrdinalPages,
    material: Material<RepositoryIdentity>,
}
impl FrozenDirectory {
    fn new(repo: SnapshotRepositoryId, host: &BoundHost, initialization: OperationId,
        rows: OrdinalPages, body: OrdinalPages) -> Result<Self> {
        let header = Header { repository_id: repo, host: host.native.clone(), initialization_operation_id: initialization,
            rows: rows.root().clone() };
        let descriptor = PreparedObject::new(Domain::DirectoryRepository, &header)?;
        let root_hash = descriptor.hash();
        let material = Material { scope: Scope::Directory,
            identity: RepositoryIdentity { repository_id: repo, directory_id: host.native.directory_id },
            logical_root: root_hash, representation: Representation::Native { root_hash },
            body_inventory: body.root().clone() };
        Ok(Self { header, descriptor, rows, body, material })
    }
    fn verify(&self, host: &BoundHost, config: &ValidatedConfig) -> Result<()> {
        host.check(config)?;
        let disk = &host.disk;
        let root = &host.handle.root;
        let actual: Header = objects::load(root, Domain::DirectoryRepository, self.descriptor.hash())?;
        check(actual == self.header && actual.host == host.native)?;
        self.rows.verify(disk, root, &actual.rows)?;
        self.body.verify(disk, root, &self.material.body_inventory)?;
        host.check(config)
    }
    fn stage(&self, host: &BoundHost, config: &ValidatedConfig) -> Result<()> {
        host.check(config)?;
        self.rows.install(&host.disk, &host.handle.root)?;
        self.body.install(&host.disk, &host.handle.root)?;
        self.descriptor.install(&host.handle.root)?;
        self.verify(host, config)
    }
    fn bytes(&self) -> Result<usize> {
        self.rows.bytes().checked_add(self.body.bytes()).and_then(|n| n.checked_add(self.descriptor.bytes()))
            .filter(|n| *n <= 64 * 1024 * 1024).ok_or_else(|| Failure::Budget.into())
    }
}
