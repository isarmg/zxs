//! A non-publishing, Initial-only owned planning snapshot. Unlike workspace
//! preparation it never holds a global mutation fence: an emergency mode change
//! must be able to win while a worker reads/copies a large body.
use crate::engine::{self, Engine, PlanningContext};
use crate::{Result, State, ValidatedConfig};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

pub(super) struct Frozen {
    root: PathBuf,
    config: ValidatedConfig,
    resources: Arc<crate::disk::DiskResources>,
    state: Arc<State>,
    original: Option<Arc<engine::InstalledOriginalReceipts>>,
    names: engine::named_namespaces::NameCache,
    baselines: BTreeMap<String, engine::WorkspaceNamespaceBaseline>,
    snapshot: Option<String>,
    sequence: u64,
    integrity: Arc<engine::provenance::MetadataIntegrity>,
    _locks: crate::disk::WriterLocks,
}
impl Frozen {
    pub(super) fn capture(engine: &Engine) -> Result<Self> {
        let context = engine.planning_context();
        context.require_write()?;
        if context.current_namespaces.is_some() || context.config.generation().is_some() {
            return Err(crate::business::file_draft::automatic::stale());
        }
        Ok(Self { root: context.root.to_owned(), config: context.config.clone(),
            resources: Arc::clone(context.disk_resources), state: Arc::clone(context.state),
            original: context.original_receipts.cloned(), names: context.named_namespaces.seed(),
            baselines: BTreeMap::new(), snapshot: context.snapshot.clone(), sequence: context.next_slot,
            integrity: Arc::clone(context.metadata_integrity), _locks: engine.retain_writer_locks()? })
    }
    pub(super) fn context(&self) -> PlanningContext<'_> {
        PlanningContext { root: &self.root, config: &self.config, disk_resources: &self.resources,
            state: &self.state, original_receipts: self.original.as_ref(), named_namespaces: &self.names,
            current_namespaces: None, workspace_namespace_baselines: &self.baselines, snapshot: &self.snapshot,
            next_slot: self.sequence, blocked: false, metadata_integrity: &self.integrity,
            admission: engine::planning::Admission::Detached, integrity_scrub_schedule: None,
            published_state_base: None }
    }
}
