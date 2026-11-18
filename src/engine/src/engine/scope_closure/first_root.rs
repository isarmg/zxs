//! First ordinary filesystem repository on a genuinely empty fixed genesis.
//! Its original operation is fully reprojected; no arbitrary zero-inventory
//! rule or new authority. This package cannot select a published frontier.
use super::{Anchor, OrdinalBuilder, OrdinalPages, PreparedImage, ScopeClosureError as Failure,
    genesis::{MAX_DISKS,PAGE_LIMIT,registration_key}, objects::{Domain,PreparedObject},
    schema::{Closure,Counts,DiskManifest,Material,Membership,Origin,Scope,Totals}, system::FrozenSystem};
use crate::{DiskConfig, Result, disk::{DiskHandle,DiskRoot}, engine::{disk_catalog::{FrozenDiskPair,PreparedDiskScopes,Predecessor},
    fresh_layout, index::preparation::SystemEncodingInput, publication::FirstRootScopeInput}, journal::PreparedMetadataObject,
    metadata_map::{Encoder,ProofMeasure}};
use std::{borrow::Cow,collections::BTreeMap,sync::{Arc,atomic::{AtomicBool,Ordering}}};
use storage_contract::{ContentHash,DiskId,Sequence,working_tree::MaterialCommitment};

const MAX: usize = 64 * 1024 * 1024;
fn add(n: &mut usize, x: usize) -> Result<()> {
    *n = n.checked_add(x).filter(|sum| *sum <= MAX).ok_or(Failure::Budget)?;
    Ok(())
}
fn running(stop: &AtomicBool) -> Result<()> {
    if stop.load(Ordering::Acquire) { return Err(crate::Error::new(crate::ErrorCode::OperationAborted, "stopped first root preparation")); }
    Ok(())
}
/// The canonical eighth field is part of this actual Snapshot object, not a
/// Candidate amendment, config marker or parallel vote. All seven original
/// state fields are borrowed from the completed native encoder unchanged.
#[derive(data_serialization::Serialize)]
struct Descriptor<'a> {
    #[serialization(flatten)] state: &'a crate::journal::Snapshot,
    scope_closure: MaterialCommitment,
}
struct Local { disk: DiskConfig, held: Arc<DiskHandle>, pair: FrozenDiskPair, manifest: PreparedObject }
pub(in crate::engine) struct FirstRootGraph<'a> {
    source: FirstRootScopeInput<'a>,
    system: DiskRoot, system_entity: platform_io::fs_safety::linux::FileIdentity,
    system_descriptor: PreparedObject, system_image: PreparedImage, empty: OrdinalPages,
    disk_index: Vec<PreparedMetadataObject>, disks: BTreeMap<DiskId,Local>, root: PreparedObject,
    descriptor: PreparedMetadataObject,
}
impl<'a> FirstRootGraph<'a> {
    pub(in crate::engine) fn capture(source: FirstRootScopeInput<'a>, stop: &AtomicBool) -> Result<Self> {
        running(stop)?;
        source.require()?;
        let engine = source.engine();
        let cfg = &engine.config;
        super::require_disk_budget(cfg)?;
        let fixed = cfg.fixed_genesis()?;
        let original = Anchor::decode(fixed.bytes())?;
        if *original != **cfg || cfg.storage_roots.len() != cfg.disks.len() || cfg.disks.len() > MAX_DISKS {
            return Err(Failure::GenesisIdentity.into());
        }
        let system = engine._locks.held_scope_system()?;
        let system_entity = system.directory_identity(system.path())?;
        Anchor::verify_system(cfg,&system)?;
        // Inspect every registered holder, not just the fixed metadata voters.
        // Completeness also follows independently from the native mutation.
        for disk in &cfg.disks {
            running(stop)?;
            let holder = engine.disk_resources.get(disk)?;
            Anchor::verify_original_disk(cfg,disk,&holder.root)?;
            empty_business_areas(holder)?;
        }
        let configuration = MaterialCommitment::from_digest(storage_contract::canonical_object_digest("pool-config",&**cfg)?);
        let r = source.receipt();
        let sequence = Sequence::new(r.sequence);
        let origin = Origin::Publication { operation_id: r.operation_id,
            request_digest: MaterialCommitment::from_digest(r.request_digest.parse::<ContentHash>()
                .map_err(|_| Failure::PublicationMismatch)?.digest()), predecessor: Predecessor::Genesis { anchor_hash: fixed.hash } };
        let empty = OrdinalBuilder::new(0).finish()?;
        let (system_descriptor,system_image,sys_size) =
            FrozenSystem::first_root(&source,empty.root(),configuration,&|| running(stop))?.into_parts();
        let mut allocated = 0;
        for n in [empty.bytes(),system_descriptor.bytes(),system_image.bytes()] { add(&mut allocated,n)?; }
        let mut counts = Totals { disks: cfg.disks.len() as u64, inventory_entries: 0, materials: 1,
            body_entries: 0, canonical_metadata_bytes: sys_size as u64 };
        let mut index = BTreeMap::new();
        let mut disks = BTreeMap::new();
        for pair in PreparedDiskScopes::new(cfg)?.freeze_next(&engine.state,source.next(),None,r,stop)? {
            let id = pair.registration.disk_id.parse::<DiskId>()?;
            let binding = cfg.storage_roots.get(&id).ok_or(Failure::GenesisIdentity)?;
            let disk = cfg.disks.iter().find(|d| d.disk_id == pair.registration.disk_id).ok_or(Failure::GenesisIdentity)?;
            if pair.registration != binding.registration || disks.contains_key(&id) { return Err(Failure::PublicationMismatch.into()); }
            pair.require_planned_publication(cfg,None,r)?;
            let material = |scope,(logical_root,representation)| Material { scope,identity: pair.registration.clone(),
                logical_root,representation,body_inventory:empty.root().clone() };
            let current = material(Scope::DiskCurrent,pair.current());
            let history = material(Scope::DiskHistory,pair.history());
            let size = storage_contract::canonical_json(&current)?.len() + storage_contract::canonical_json(&history)?.len();
            let manifest = PreparedObject::new(Domain::Disk,&DiskManifest {
                registration:pair.registration.clone(),root_binding:binding.clone(),origin,sequence,
                inventory:empty.root().clone(), disk_current:current, disk_history:history, local_changes:pair.local_changes().clone(),
                retirement_obligations:empty.root().clone(),
                counts:Counts { inventory_entries:0,materials:2,body_entries:0,canonical_metadata_bytes:size as u64 },
            })?;
            add(&mut allocated,pair.bytes())?; add(&mut allocated,manifest.bytes())?;
            counts.materials += 2; counts.canonical_metadata_bytes += size as u64;
            index.insert(registration_key(&pair.registration)?,json_codec::to_value(manifest.hash())?);
            disks.insert(id,Local {disk:disk.clone(),held:Arc::clone(engine.disk_resources.get(disk)?),pair,manifest});
        }
        if disks.len() != cfg.disks.len() { return Err(Failure::PublicationMismatch.into()); }
        let mut map = Encoder::new(PAGE_LIMIT);
        let route = map.map_typed(index,ProofMeasure::Plain,&|s: &String| Cow::Borrowed(s.as_str()))?;
        let root = PreparedObject::new(Domain::Closure,&Closure { pool_id:cfg.pool_id.parse()?,origin,sequence,
            membership:Membership::Genesis {config_hash:configuration},system_material:system_descriptor.hash(),disks:route.reference().clone(),counts })?;
        add(&mut allocated,root.bytes())?;
        let disk_index = map.finish();
        for p in &disk_index { add(&mut allocated,p.bytes().len())?; }
        let state = source.state_descriptor()?;
        crate::journal::validate_snapshot_schema(state)?;
        if state.sequence != sequence.get() { return Err(Failure::PublicationMismatch.into()); }
        let descriptor = PreparedMetadataObject::new("snapshot",&Descriptor { state, scope_closure: root.hash() },crate::disk::MAX_METADATA)?;
        add(&mut allocated,descriptor.bytes().len())?;
        let graph = Self {source,system,system_entity,system_descriptor,system_image,empty,disk_index,disks,root,descriptor};
        graph.require(stop)?;
        Ok(graph)
    }

    fn require(&self,stop: &AtomicBool) -> Result<()> {
        running(stop)?;
        self.source.require()?;
        let e = self.source.engine();
        let current = e._locks.held_scope_system()?;
        if current.directory_identity(current.path())? != self.system_entity
            || self.system.directory_identity(self.system.path())? != self.system_entity { return Err(Failure::PublicationMismatch.into()); }
        Anchor::verify_system(&e.config,&current)?;
        let fixed = e.config.fixed_genesis()?.hash;
        for d in self.disks.values() {
            running(stop)?;
            if Anchor::read(&d.held.root)?.fixed_genesis()?.hash != fixed { return Err(Failure::GenesisIdentity.into()); }
            if !Arc::ptr_eq(&d.held,e.disk_resources.get(&d.disk)?) { return Err(Failure::PublicationMismatch.into()); }
            fresh_layout::verify_root(&e.config,&d.disk,&d.held.root)?;
            empty_business_areas(&d.held)?;
        }
        Ok(())
    }
    pub(in crate::engine) fn commitment(&self) -> MaterialCommitment { self.root.hash() }

    /// The full new root used by the eventual existing native decision,
    /// separate from (and not substitutable for) the inner closure digest.
    pub(in crate::engine) fn snapshot_object(&self) -> &PreparedMetadataObject { &self.descriptor }

    /// Only after a full package preflight: write non-secret private immutable
    /// addresses and verify through the retained originals. No D3, WAL, alias,
    /// original Candidate, or caller-visible publication is created here.
    pub(in crate::engine) fn stage_original(&self,stop: &AtomicBool) -> Result<()> {
        self.require(stop)?;
        let cfg = &self.source.engine().config;
        let system = SystemEncodingInput::first_root(&self.source,&|| running(stop))?;
        self.system_image.install_verified(&self.system,|b| system.verify(b,&|| running(stop)))?;
        self.system_descriptor.install(&self.system)?;
        // The actual empty body page already belongs to the fixed SYSTEM
        // genesis and is not replicated from another holder.
        Anchor::verify_system(cfg,&self.system)?;
        self.root.install(&self.system)?;
        for d in self.disks.values() {
            running(stop)?;
            let held = &d.held.root;
            d.pair.install_at(cfg,&d.disk,held)?;
            self.empty.install(&d.disk,held)?;
            d.manifest.install(held)?;
            for p in &self.disk_index { p.store_at_root(held,&d.disk)?; }
            self.root.install(held)?;
            fresh_layout::verify_root(cfg,&d.disk,held)?;
        }
        self.descriptor.store_at(&self.source.engine().disk_resources,&self.source.engine().members())?;
        self.require(stop)
    }
}
fn empty_business_areas(holder: &DiskHandle) -> Result<()> {
    // Reserved metadata and staging are NOT classified as business objects
    // or cleaned. These four bounded probes cover the registered user roles.
    for root in [holder.content_root()?,holder.deleted_root()?,holder.current_management_root()?,holder.retained_management_root()?] {
        if root.entries(root.path())?.next().transpose()?.is_some() { return Err(Failure::PublicationMismatch.into()); }
    }
    Ok(())
}


#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/scope_first_root.rs"]
mod tests;
